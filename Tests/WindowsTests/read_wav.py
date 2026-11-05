"""Independent standard-library reader for the Rust-created synthetic WAV."""
import sys,wave
with wave.open(sys.argv[1],'rb') as w:
 assert (w.getnchannels(),w.getsampwidth(),w.getframerate(),w.getnframes())==(1,2,16000,3)
 assert w.readframes(3)==b'\x00\x80\x00\x00\xff\x7f'
print('Python wave decoded exact Rust PCM frames')
