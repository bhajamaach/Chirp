//! Microphone capture. The stream only exists while a recording is running, so
//! the mic is off (and macOS shows no recording indicator) between utterances.

use std::{
    sync::mpsc,
    thread::{self, JoinHandle},
    time::Duration,
};

use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    ErrorKind, FromSample, Sample, SampleFormat, SizedSample,
};

use super::audio::{downmix, to_i16, FrameChunker, Resampler, TARGET_RATE};

/// A running capture. Stopping it ends the frame stream returned by `start`.
pub struct Recording {
    stop_tx: mpsc::Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl Recording {
    /// Opens the default microphone. Returns once audio is flowing, or why it
    /// failed. Frames of 16 kHz mono 16-bit audio, 256 samples each, arrive on
    /// the receiver until the recording is stopped.
    pub fn start() -> Result<(Recording, mpsc::Receiver<Vec<i16>>), String> {
        let (frames_tx, frames_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        let (stop_tx, stop_rx) = mpsc::channel::<()>();

        // The cpal stream is not Send, so it lives and dies on this thread.
        let thread = thread::Builder::new()
            .name("chirp-mic".into())
            .spawn(move || match build_stream(frames_tx) {
                Ok(stream) => match stream.play() {
                    Ok(()) => {
                        let _ = ready_tx.send(Ok(()));
                        // Returns when stop() is called or the Recording is dropped.
                        let _ = stop_rx.recv();
                        drop(stream);
                    }
                    Err(error) => {
                        let _ = ready_tx.send(Err(format!("could not start the microphone: {error}")));
                    }
                },
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                }
            })
            .map_err(|e| format!("could not start the microphone thread: {e}"))?;

        match ready_rx.recv_timeout(Duration::from_secs(3)) {
            Ok(Ok(())) => Ok((Recording { stop_tx, thread: Some(thread) }, frames_rx)),
            Ok(Err(error)) => Err(error),
            Err(_) => Err("the microphone did not start in time".into()),
        }
    }

    /// Stops the microphone. Frames already captured stay readable on the
    /// receiver, which then ends.
    pub fn stop(&mut self) {
        let _ = self.stop_tx.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        self.stop();
    }
}

fn build_stream(tx: mpsc::Sender<Vec<i16>>) -> Result<cpal::Stream, String> {
    let host = cpal::default_host();
    let device = host.default_input_device().ok_or("no microphone found")?;
    let config = device
        .default_input_config()
        .map_err(|e| format!("could not read the microphone settings: {e}"))?;
    let rate: u32 = config.sample_rate();
    let channels = config.channels() as usize;
    match config.sample_format() {
        SampleFormat::F32 => typed::<f32>(&device, config.into(), rate, channels, tx),
        SampleFormat::I16 => typed::<i16>(&device, config.into(), rate, channels, tx),
        SampleFormat::I32 => typed::<i32>(&device, config.into(), rate, channels, tx),
        other => Err(format!("unsupported microphone format {other}")),
    }
}

fn typed<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    rate: u32,
    channels: usize,
    tx: mpsc::Sender<Vec<i16>>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    let mut resampler = Resampler::new(rate, TARGET_RATE);
    let mut chunker = FrameChunker::default();
    let (mut floats, mut mono, mut resampled) = (Vec::new(), Vec::new(), Vec::new());

    device
        .build_input_stream(
            config,
            move |data: &[T], _: &_| {
                floats.clear();
                floats.extend(data.iter().map(|s| f32::from_sample(*s)));
                mono.clear();
                downmix(&floats, channels, &mut mono);
                resampled.clear();
                resampler.process(&mono, &mut resampled);
                let pcm: Vec<i16> = resampled.iter().map(|s| to_i16(*s)).collect();
                chunker.push(&pcm, |frame| {
                    let _ = tx.send(frame);
                });
            },
            |error| match error.kind() {
                // Brief glitches the stream recovers from, common on the first press.
                ErrorKind::Xrun | ErrorKind::DeviceChanged | ErrorKind::RealtimeDenied => {
                    eprintln!("[chirp] microphone glitch (the stream continues): {error}")
                }
                _ => eprintln!("[chirp] microphone error: {error}"),
            },
            None,
        )
        .map_err(|e| format!("could not open the microphone: {e}"))
}
