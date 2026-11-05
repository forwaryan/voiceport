//! Bounded mono 16-bit PCM WAV encoding, not a device recorder.
use std::io::{self, Seek, SeekFrom, Write};
pub const SAMPLE_RATE: u32 = 16_000;
pub const MAX_SAMPLES: usize = SAMPLE_RATE as usize * 180;
pub struct Wav<W: Write + Seek> {
    out: W,
    samples: usize,
    poisoned: bool,
}
impl<W: Write + Seek> Wav<W> {
    pub fn new(mut out: W) -> io::Result<Self> {
        if out.stream_position()? != 0 || out.seek(SeekFrom::End(0))? != 0 {
            return Err(io::Error::other("WAV must start at offset zero"));
        }
        out.write_all(&[0; 44])?;
        Ok(Self {
            out,
            samples: 0,
            poisoned: false,
        })
    }
    pub fn append(&mut self, samples: &[i16]) -> io::Result<()> {
        if self.poisoned || samples.len() > MAX_SAMPLES - self.samples {
            return Err(io::Error::other("Recording duration exceeded"));
        }
        self.poisoned = true;
        for chunk in samples.chunks(4096) {
            let mut bytes = [0u8; 8192];
            for (i, s) in chunk.iter().enumerate() {
                bytes[i * 2..i * 2 + 2].copy_from_slice(&s.to_le_bytes());
            }
            self.out.write_all(&bytes[..chunk.len() * 2])?;
        }
        self.poisoned = false;
        self.samples += samples.len();
        Ok(())
    }
    pub fn finish(mut self) -> io::Result<W> {
        if self.poisoned || self.samples == 0 {
            return Err(io::Error::other("Empty recording"));
        }
        let n = (self.samples * 2) as u32;
        self.out.seek(SeekFrom::Start(0))?;
        self.out.write_all(b"RIFF")?;
        self.out.write_all(&(n + 36).to_le_bytes())?;
        self.out.write_all(b"WAVEfmt ")?;
        self.out.write_all(&16u32.to_le_bytes())?;
        self.out.write_all(&1u16.to_le_bytes())?;
        self.out.write_all(&1u16.to_le_bytes())?;
        self.out.write_all(&SAMPLE_RATE.to_le_bytes())?;
        self.out.write_all(&(SAMPLE_RATE * 2).to_le_bytes())?;
        self.out.write_all(&2u16.to_le_bytes())?;
        self.out.write_all(&16u16.to_le_bytes())?;
        self.out.write_all(b"data")?;
        self.out.write_all(&n.to_le_bytes())?;
        self.out.seek(SeekFrom::End(0))?;
        self.out.flush()?;
        Ok(self.out)
    }
}
