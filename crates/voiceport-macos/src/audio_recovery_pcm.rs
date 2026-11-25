//! A replacement tap may have a different rate or channel layout. Normalize
//! only that tap to the active capture format so the writer and ASR keep their
//! original session, settings, and sample-rate budget.
use super::{MAXIMUM_TAP_FRAMES, native_error, validate_format};
use block2::RcBlock;
use objc2::{AllocAnyThread, rc::Retained};
use objc2_avf_audio::{
    AVAudioBuffer, AVAudioConverter, AVAudioConverterInputStatus, AVAudioConverterOutputStatus,
    AVAudioConverterPrimeMethod, AVAudioFormat, AVAudioPCMBuffer,
};
use std::{
    ptr::NonNull,
    sync::atomic::{AtomicBool, Ordering},
};

pub(super) struct FormatBridge {
    converter: Retained<AVAudioConverter>,
    input: Retained<AVAudioFormat>,
    output: Retained<AVAudioPCMBuffer>,
}
// SAFETY: Constructed before publication; exclusively used under a Mutex in the
// replacement tap. None of its retained objects are shared with another tap.
unsafe impl Send for FormatBridge {}

impl FormatBridge {
    pub fn new(input: &AVAudioFormat, target: &AVAudioFormat) -> Result<Option<Self>, String> {
        validate_format(input)?;
        if input == target {
            return Ok(None);
        }
        let converter = unsafe {
            AVAudioConverter::initFromFormat_toFormat(AVAudioConverter::alloc(), input, target)
        }
        .ok_or("备用设备无法转换到本轮录音的音频格式。")?;
        unsafe {
            converter.setDownmix(true);
            converter.setPrimeMethod(AVAudioConverterPrimeMethod::None);
        }
        let output = unsafe {
            AVAudioPCMBuffer::initWithPCMFormat_frameCapacity(
                AVAudioPCMBuffer::alloc(),
                target,
                crate::local_audio::MAX_PACKET_FRAMES as u32,
            )
        }
        .ok_or("无法为备用麦克风分配缓冲区。")?;
        // AVAudioFormat is immutable and the callback may outlive the node.
        let input = unsafe { Retained::retain(std::ptr::from_ref(input).cast_mut()).unwrap() };
        Ok(Some(Self {
            converter,
            input,
            output,
        }))
    }

    pub fn convert(
        &mut self,
        buffer: &AVAudioPCMBuffer,
        mut consume: impl FnMut(&AVAudioPCMBuffer),
    ) -> Result<(), String> {
        unsafe {
            if buffer.format() != self.input
                || buffer.frameLength() > MAXIMUM_TAP_FRAMES
                || buffer.frameLength() > buffer.frameCapacity()
            {
                return Err("备用麦克风的采样格式发生变化。".into());
            }
            if buffer.frameLength() == 0 {
                return Ok(());
            }
        }
        let input = unsafe { Retained::retain(std::ptr::from_ref(buffer).cast_mut()).unwrap() };
        let supplied = AtomicBool::new(false);
        let feed = RcBlock::new(
            move |_packets: u32,
                  mut status: NonNull<AVAudioConverterInputStatus>|
                  -> *mut AVAudioBuffer {
                if !supplied.swap(true, Ordering::AcqRel) {
                    unsafe { *status.as_mut() = AVAudioConverterInputStatus::HaveData };
                    return Retained::as_ptr(&input).cast_mut().cast();
                }
                unsafe { *status.as_mut() = AVAudioConverterInputStatus::NoDataNow };
                std::ptr::null_mut()
            },
        );
        for _ in 0..32 {
            let mut error = None;
            let status = unsafe {
                self.output.setFrameLength(0);
                self.converter.convertToBuffer_error_withInputFromBlock(
                    &self.output,
                    Some(&mut error),
                    RcBlock::as_ptr(&feed),
                )
            };
            if let Some(error) = error {
                return Err(native_error(error));
            }
            if status == AVAudioConverterOutputStatus::Error {
                return Err("备用麦克风采样转换失败。".into());
            }
            if unsafe { self.output.frameLength() } != 0 {
                consume(&self.output);
            }
            if status == AVAudioConverterOutputStatus::InputRanDry {
                return Ok(());
            }
            if status != AVAudioConverterOutputStatus::HaveData {
                break;
            }
        }
        Err("备用麦克风采样转换无法继续。".into())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn replacement_bluetooth_format_feeds_original_stream_and_aac() {
        use super::*;
        use crate::audio::{copy_pcm, tests::synthetic_buffer};
        let (narrow, samples) = synthetic_buffer(16_000., 1, 1024);
        let (wide, _) = synthetic_buffer(48_000., 2, 1024);
        assert!(FormatBridge::new(&wide, &wide).unwrap().is_none());
        let mut bridge = FormatBridge::new(&narrow, &wide).unwrap().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("synthetic.m4a");
        let (tx, rx) = crate::local_audio::channel();
        let mut writer = super::super::AacWriter::new(&file, &wide).unwrap();
        let mut frames = 0;
        for _ in 0..3 {
            bridge
                .convert(&samples, |output| {
                    writer.write(output).unwrap();
                    let packet = copy_pcm(output).unwrap();
                    assert_eq!(packet.rate, 48_000.);
                    assert_eq!(packet.channels, 2);
                    frames += packet.samples.len() / 2;
                    tx.send(packet).unwrap();
                })
                .unwrap();
        }
        assert!(
            (8_000..=9_300).contains(&frames),
            "resampled frames: {frames}"
        );
        assert!(
            rx.receiver
                .try_recv()
                .unwrap()
                .samples
                .iter()
                .any(|&s| s != 0.0)
        );
        writer.finish().unwrap();
        super::super::validate_aac(&file).unwrap();
    }
}
