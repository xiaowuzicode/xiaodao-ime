//! 录音：cpal 输入流 → 16kHz 单声道 f32；实现 [`crate::types::Recorder`]。
//!
//! 对齐 Python 版 `xiaodao_ime/recorder.py`（start/stop/abort/snapshot/level 语义一致），
//! 两点刻意不同：(1) **时长按「样本数 / 16000」算，不用墙钟**——macOS 未授麦克风权限时流能
//! 开起来但零数据，按样本数算得 0s，正好被上层「太短就丢弃」的门槛拦下；(2) 设备不支持
//! 16kHz 单声道 f32 时不再直接失败，改用设备默认配置，回调里混单声道 + 重采样到 16k。
//!
//! 线程模型：cpal 的 Stream 在 macOS / Windows 上都不是 Send，所以流由一条专属线程持有，
//! 主线程通过 crossbeam 通道下达收尾信号，线程 drop 掉流即关闭设备；音频回调只做
//! 「格式转换 → 混音 → 重采样 → 加锁 push」，没有阻塞 I/O。

use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::types::Recorder;
use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SampleRate, StreamConfig, SupportedStreamConfig};
use crossbeam_channel::{bounded, Sender};
use parking_lot::Mutex;
use rubato::{
    calculate_cutoff, Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};
use tracing::{debug, error, info, warn};

/// 转写引擎要求的采样率（SenseVoice 固定 16kHz 单声道）。
pub const TARGET_SAMPLE_RATE: u32 = 16_000;
/// level() 的统计窗口（秒），与 Python 版一致。
const LEVEL_WINDOW_SECONDS: f64 = 0.12;
/// 重采样每批的输入帧数；取小值让「不足一批」的尾巴损失可忽略（48kHz 下 256 帧 ≈ 5ms）。
const RESAMPLE_CHUNK: usize = 256;
const OPEN_TIMEOUT: Duration = Duration::from_secs(5);
/// 16kHz 单声道 f32 累积缓冲：录音回调与主线程之间唯一的共享状态，可脱离真实设备单测。
#[derive(Debug, Default)]
pub struct PcmBuffer {
    samples: Vec<f32>,
}

impl PcmBuffer {
    pub fn new() -> Self {
        // 预留 30s，避免长录音反复扩容
        Self {
            samples: Vec::with_capacity(TARGET_SAMPLE_RATE as usize * 30),
        }
    }
    /// 追加一批 16k 单声道样本（录音回调调用，持锁时间只有一次 memcpy）。
    pub fn push(&mut self, samples: &[f32]) {
        self.samples.extend_from_slice(samples);
    }
    pub fn clear(&mut self) {
        self.samples.clear();
    }
    /// 取走整段 PCM，缓冲复位。
    pub fn take(&mut self) -> Vec<f32> {
        std::mem::take(&mut self.samples)
    }
    /// 拷贝当前累积（伪流式预览用，不影响继续录音）。
    pub fn snapshot(&self) -> Vec<f32> {
        self.samples.clone()
    }
    /// 已录时长（秒）= 样本数 / 16000。
    pub fn duration(&self) -> f64 {
        self.samples.len() as f64 / f64::from(TARGET_SAMPLE_RATE)
    }
    /// 最近 ~120ms 的响度（0~1）。映射公式抄 Python 版：语音 RMS 典型 0.005~0.2，
    /// 放大 14 倍后开方压缩，让小音量也有可见波动。
    pub fn level(&self) -> f32 {
        let window = (f64::from(TARGET_SAMPLE_RATE) * LEVEL_WINDOW_SECONDS) as usize;
        let seg = &self.samples[self.samples.len().saturating_sub(window)..];
        if seg.is_empty() {
            return 0.0;
        }
        let mean_square = seg
            .iter()
            .map(|v| f64::from(*v) * f64::from(*v))
            .sum::<f64>()
            / seg.len() as f64;
        ((mean_square.sqrt() * 14.0).sqrt() as f32).min(1.0)
    }
}

/// 交错多声道 → 单声道（逐帧取平均）；不足一帧的尾巴丢弃。
pub fn mix_to_mono(interleaved: &[f32], channels: usize, out: &mut Vec<f32>) {
    if channels <= 1 {
        out.extend_from_slice(interleaved);
        return;
    }
    let scale = 1.0 / channels as f32;
    for frame in interleaved.chunks_exact(channels) {
        out.push(frame.iter().sum::<f32>() * scale);
    }
}

/// 流式重采样器：单声道任意采样率 → 16kHz。16kHz 输入零成本直通；否则走 rubato
/// `SincFixedIn`（带抗混叠低通，48k→16k 不会把 8kHz 以上能量折回语音带）。不足一批
/// （[`RESAMPLE_CHUNK`] 帧）的尾巴留在内部，录音结束时丢弃，损失 ≤ 5ms。
pub struct Resampler16k {
    inner: Option<SincFixedIn<f32>>,
    pending: Vec<f32>,
    in_buf: Vec<Vec<f32>>,
    out_buf: Vec<Vec<f32>>,
}

impl Resampler16k {
    pub fn new(input_rate: u32) -> Result<Self> {
        if input_rate == 0 {
            return Err(anyhow!("非法采样率 0"));
        }
        if input_rate == TARGET_SAMPLE_RATE {
            return Ok(Self {
                inner: None,
                pending: Vec::new(),
                in_buf: Vec::new(),
                out_buf: Vec::new(),
            });
        }
        let sinc_len = 128;
        let params = SincInterpolationParameters {
            sinc_len,
            f_cutoff: calculate_cutoff(sinc_len, WindowFunction::BlackmanHarris2),
            oversampling_factor: 128,
            interpolation: SincInterpolationType::Quadratic,
            window: WindowFunction::BlackmanHarris2,
        };
        let ratio = f64::from(TARGET_SAMPLE_RATE) / f64::from(input_rate);
        let inner = SincFixedIn::<f32>::new(ratio, 1.0, params, RESAMPLE_CHUNK, 1)
            .map_err(|e| anyhow!("创建重采样器失败（{input_rate}Hz → 16kHz）：{e}"))?;
        let out_buf = inner.output_buffer_allocate(true);
        Ok(Self {
            inner: Some(inner),
            pending: Vec::with_capacity(RESAMPLE_CHUNK * 2),
            in_buf: vec![Vec::with_capacity(RESAMPLE_CHUNK)],
            out_buf,
        })
    }

    /// 吃进一批单声道样本，把转换好的 16k 样本追加到 `out`。
    pub fn process(&mut self, mono: &[f32], out: &mut Vec<f32>) {
        let Some(inner) = self.inner.as_mut() else {
            out.extend_from_slice(mono);
            return;
        };
        self.pending.extend_from_slice(mono);
        while self.pending.len() >= RESAMPLE_CHUNK {
            self.in_buf[0].clear();
            self.in_buf[0].extend(self.pending.drain(..RESAMPLE_CHUNK));
            match inner.process_into_buffer(&self.in_buf, &mut self.out_buf, None) {
                Ok((_, written)) => out.extend_from_slice(&self.out_buf[0][..written]),
                Err(e) => warn!("重采样失败，丢弃 {RESAMPLE_CHUNK} 帧：{e}"),
            }
        }
    }
}

/// 回调侧转换流水线：交错样本 → f32 → 单声道 → 16kHz。
struct Pipeline {
    channels: usize,
    interleaved: Vec<f32>,
    mono: Vec<f32>,
    resampled: Vec<f32>,
    resampler: Resampler16k,
}

impl Pipeline {
    fn new(channels: usize, input_rate: u32) -> Result<Self> {
        Ok(Self {
            channels: channels.max(1),
            interleaved: Vec::with_capacity(4096),
            mono: Vec::with_capacity(4096),
            resampled: Vec::with_capacity(4096),
            resampler: Resampler16k::new(input_rate)?,
        })
    }

    fn feed<T>(&mut self, data: &[T], buffer: &Mutex<PcmBuffer>)
    where
        T: Sample,
        f32: FromSample<T>,
    {
        self.interleaved.clear();
        self.interleaved
            .extend(data.iter().map(|s| f32::from_sample(*s)));
        self.mono.clear();
        mix_to_mono(&self.interleaved, self.channels, &mut self.mono);
        self.resampled.clear();
        self.resampler.process(&self.mono, &mut self.resampled);
        if !self.resampled.is_empty() {
            buffer.lock().push(&self.resampled);
        }
    }
}

struct StreamSession {
    stop_tx: Sender<()>,
    join: JoinHandle<()>,
}

/// 基于 cpal 的录音器。`new()` 不碰设备，`start()` 才打开麦克风。
pub struct CpalRecorder {
    buffer: Arc<Mutex<PcmBuffer>>,
    session: Option<StreamSession>,
}

impl Default for CpalRecorder {
    fn default() -> Self {
        Self::new()
    }
}

impl CpalRecorder {
    pub fn new() -> Self {
        Self {
            buffer: Arc::new(Mutex::new(PcmBuffer::new())),
            session: None,
        }
    }

    /// 通知持流线程收尾并等它退出（drop 掉 Stream 即关闭设备）。
    fn shutdown(&mut self) {
        let Some(session) = self.session.take() else {
            return;
        };
        let _ = session.stop_tx.send(());
        if session.join.join().is_err() {
            error!("录音线程异常退出（panic）");
        }
    }
}

impl Recorder for CpalRecorder {
    fn start(&mut self) -> Result<()> {
        if self.session.is_some() {
            debug!("录音已在进行，忽略重复 start");
            return Ok(());
        }
        self.buffer.lock().clear();

        let buffer = Arc::clone(&self.buffer);
        let (ready_tx, ready_rx) = bounded::<std::result::Result<String, String>>(1);
        let (stop_tx, stop_rx) = bounded::<()>(1);
        let join = std::thread::Builder::new()
            .name("xiaodao-audio".to_string())
            .spawn(move || match open_input_stream(buffer) {
                Ok((stream, desc)) => {
                    if let Err(e) = stream.play() {
                        let _ = ready_tx.send(Err(format!("启动输入流失败：{e}")));
                        return;
                    }
                    let _ = ready_tx.send(Ok(desc));
                    // 阻塞到收尾信号（发送端析构也会解除阻塞），随后 drop 流关闭设备
                    let _ = stop_rx.recv();
                    drop(stream);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(format!("{e:#}")));
                }
            })
            .context("创建录音线程失败")?;

        match ready_rx.recv_timeout(OPEN_TIMEOUT) {
            Ok(Ok(desc)) => {
                info!("录音开始（{desc}）");
                self.session = Some(StreamSession { stop_tx, join });
                Ok(())
            }
            Ok(Err(msg)) => {
                let _ = join.join();
                error!("打开录音设备失败：{msg}");
                Err(anyhow!("打开录音设备失败：{msg}"))
            }
            Err(_) => {
                let _ = stop_tx.send(());
                let _ = join.join();
                error!("打开录音设备超时（超过 {OPEN_TIMEOUT:?}）");
                Err(anyhow!("打开录音设备超时"))
            }
        }
    }

    fn stop(&mut self) -> (Vec<f32>, f64) {
        if self.session.is_none() {
            return (Vec::new(), 0.0);
        }
        self.shutdown();
        let pcm = self.buffer.lock().take();
        let duration = pcm.len() as f64 / f64::from(TARGET_SAMPLE_RATE);
        if pcm.is_empty() {
            warn!("录音停止：无音频数据（检查麦克风权限是否已授予）");
        }
        info!("录音停止：时长 {duration:.3}s，样本数 {}", pcm.len());
        (pcm, duration)
    }

    fn abort(&mut self) {
        if self.session.is_none() {
            return;
        }
        self.shutdown();
        let mut buffer = self.buffer.lock();
        let duration = buffer.duration();
        buffer.clear();
        info!("录音取消：已丢弃，时长 {duration:.3}s");
    }

    fn snapshot(&self) -> Vec<f32> {
        self.buffer.lock().snapshot()
    }

    fn duration(&self) -> f64 {
        self.buffer.lock().duration()
    }

    fn level(&self) -> f32 {
        self.buffer.lock().level()
    }

    fn is_recording(&self) -> bool {
        self.session.is_some()
    }
}

/// 打开默认输入设备的输入流（在持流线程里调用）。
fn open_input_stream(buffer: Arc<Mutex<PcmBuffer>>) -> Result<(cpal::Stream, String)> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| anyhow!("找不到默认输入设备（麦克风）"))?;
    let name = device.name().unwrap_or_else(|_| "未知设备".to_string());
    let supported =
        pick_input_config(&device).with_context(|| format!("查询输入设备「{name}」配置失败"))?;
    let sample_format = supported.sample_format();
    let channels = supported.channels() as usize;
    let input_rate = supported.sample_rate().0;
    let config: StreamConfig = supported.config();
    let desc =
        format!("设备={name}，{input_rate}Hz/{channels}声道/{sample_format:?} → 16kHz 单声道");
    let pipeline = Pipeline::new(channels, input_rate)?;

    let stream = match sample_format {
        SampleFormat::F32 => build_stream::<f32>(&device, &config, pipeline, buffer),
        SampleFormat::I8 => build_stream::<i8>(&device, &config, pipeline, buffer),
        SampleFormat::I16 => build_stream::<i16>(&device, &config, pipeline, buffer),
        SampleFormat::I32 => build_stream::<i32>(&device, &config, pipeline, buffer),
        SampleFormat::U8 => build_stream::<u8>(&device, &config, pipeline, buffer),
        SampleFormat::U16 => build_stream::<u16>(&device, &config, pipeline, buffer),
        other => return Err(anyhow!("不支持的采样格式：{other:?}")),
    }
    .with_context(|| format!("打开输入流失败（{desc}）"))?;
    Ok((stream, desc))
}

/// 优先 16kHz 单声道 f32；设备不支持就退回设备默认配置。
fn pick_input_config(device: &cpal::Device) -> Result<SupportedStreamConfig> {
    if let Ok(ranges) = device.supported_input_configs() {
        for range in ranges {
            if range.sample_format() == SampleFormat::F32 && range.channels() == 1 {
                if let Some(config) = range.try_with_sample_rate(SampleRate(TARGET_SAMPLE_RATE)) {
                    return Ok(config);
                }
            }
        }
    }
    device
        .default_input_config()
        .map_err(|e| anyhow!("读取默认输入配置失败：{e}"))
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    mut pipeline: Pipeline,
    buffer: Arc<Mutex<PcmBuffer>>,
) -> std::result::Result<cpal::Stream, cpal::BuildStreamError>
where
    T: cpal::SizedSample,
    f32: FromSample<T>,
{
    device.build_input_stream::<T, _, _>(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| pipeline.feed(data, &buffer),
        |err| error!("录音流错误：{err}"),
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, freq: f32, seconds: f32) -> Vec<f32> {
        let n = (rate as f32 * seconds) as usize;
        (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin() * 0.5)
            .collect()
    }

    fn resample(rate: u32, input: &[f32]) -> Vec<f32> {
        let mut out = Vec::new();
        Resampler16k::new(rate).unwrap().process(input, &mut out);
        out
    }

    /// 缓冲语义：累积 / 快照 / 取走 / 时长（按样本数算）+ 电平映射
    /// （无数据 0、静音 0、满幅封顶 1、只看最近 120ms，公式与 Python 一致）。
    #[test]
    fn buffer_semantics_and_level() {
        let mut buf = PcmBuffer::new();
        assert!(buf.duration() == 0.0 && buf.level() == 0.0);
        buf.push(&vec![0.1_f32; 16_000]);
        buf.push(&vec![0.1_f32; 8_000]);
        assert!((buf.duration() - 1.5).abs() < 1e-9);
        assert_eq!(buf.snapshot().len(), 24_000);
        assert!(
            (buf.duration() - 1.5).abs() < 1e-9,
            "snapshot 只拷贝，不该清空"
        );
        assert_eq!(buf.take().len(), 24_000);
        assert!(buf.duration() == 0.0 && buf.level() == 0.0, "take 后应复位");

        let mut silent = PcmBuffer::new();
        silent.push(&vec![0.0_f32; 16_000]);
        assert_eq!(silent.level(), 0.0);

        let mut loud = PcmBuffer::new();
        loud.push(&vec![1.0_f32; 16_000]);
        assert_eq!(loud.level(), 1.0, "rms=1 → sqrt(14) 应被封顶到 1.0");

        let mut recent = PcmBuffer::new();
        recent.push(&vec![0.9_f32; 16_000]); // 旧数据不该影响窗口
        recent.push(&vec![0.02_f32; 1_920]); // 0.12s * 16000
        let want = (0.02_f64 * 14.0).sqrt() as f32;
        assert!((recent.level() - want).abs() < 1e-6, "{}", recent.level());
    }

    /// 混音：单声道直通、多声道取平均、半帧尾巴丢弃；
    /// 重采样：16k 直通、48k 降采样样本数约 1/3 且稳态仍是 440Hz、8k 升采样翻倍、0 报错。
    #[test]
    fn mix_and_resample_cases() {
        let mut out = Vec::new();
        mix_to_mono(&[0.1, 0.2, 0.3], 1, &mut out);
        assert_eq!(out, vec![0.1, 0.2, 0.3]);
        out.clear();
        mix_to_mono(&[1.0, 0.0, 0.5, 0.5], 2, &mut out);
        assert_eq!(out, vec![0.5, 0.5]);
        out.clear();
        mix_to_mono(&[1.0, 0.0, 0.5], 2, &mut out);
        assert_eq!(out, vec![0.5]);

        let at16k = sine(16_000, 440.0, 0.1);
        assert_eq!(resample(16_000, &at16k), at16k, "16k 输入应原样直通");

        let down = resample(48_000, &sine(48_000, 440.0, 1.0));
        assert!((15_600..=16_000).contains(&down.len()), "{}", down.len());
        let peak = down.iter().fold(0.0_f32, |m, v| m.max(v.abs()));
        assert!(peak < 0.7, "峰值异常：{peak}"); // 输入 ±0.5，不该爆掉
        let zero_cross = down[2_000..14_000]
            .windows(2)
            .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
            .count();
        // 取样区间 12000 帧 = 0.75s，440Hz 应有约 330 个上升过零
        assert!((320..=340).contains(&zero_cross), "{zero_cross} 偏离 440Hz");

        let up = resample(8_000, &sine(8_000, 300.0, 0.5));
        // 输入 4000 帧只有 15 批够整，外加 sinc 群延迟，输出略少于 8000
        assert!((7_400..=8_000).contains(&up.len()), "{}", up.len());
        assert!(Resampler16k::new(0).is_err());
    }

    /// 没 start 过时 stop/abort 不炸、返回空。
    #[test]
    fn recorder_stop_without_start_is_empty() {
        let mut rec = CpalRecorder::new();
        assert!(!rec.is_recording());
        let (pcm, dur) = rec.stop();
        assert!(pcm.is_empty() && dur == 0.0);
        rec.abort();
        assert!(rec.level() == 0.0 && rec.duration() == 0.0 && rec.snapshot().is_empty());
    }

    /// 真机测试：需要麦克风设备与权限，默认不跑。
    /// `cargo test -p xiaodao-core --lib audio -- --ignored --nocapture`
    #[test]
    #[ignore = "需要真实麦克风与系统权限"]
    fn live_record_one_second() {
        let mut rec = CpalRecorder::new();
        rec.start().expect("打开录音设备失败");
        assert!(rec.is_recording());
        std::thread::sleep(Duration::from_millis(1_000));
        let (level, snap) = (rec.level(), rec.snapshot().len());
        let (pcm, dur) = rec.stop();
        assert!(!rec.is_recording());
        println!(
            "样本数={} 时长={dur:.3}s 电平={level:.3} 快照={snap}",
            pcm.len()
        );
        assert!(pcm.len() > 8_000 && (0.5..=1.5).contains(&dur), "{dur}");
    }
}
