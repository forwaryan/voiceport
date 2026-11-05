#!/usr/bin/env python3
"""Check installed files and signature correspondence without launching GUI."""
import hashlib,json,os,pathlib,subprocess,tempfile,shutil,plistlib
import sys
sys.path.insert(0,str(pathlib.Path(__file__).resolve().parent))
from license_materials import check
ROOT=pathlib.Path(__file__).resolve().parents[1]
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def validate(app,target,swift):
 app=pathlib.Path(app);helper=app/'Contents/Helpers/voiceport-rs'
 assert helper.is_file() and helper.stat().st_mode&0o777==0o755
 main=app/'Contents/MacOS/VoicePort'
 assert main.is_file() and main.stat().st_mode&0o777==0o755
 assert (app/'Contents/Info.plist').read_bytes()==(ROOT/'resources/Info.plist').read_bytes(),'unexpected bundle Info'
 assert plistlib.loads((app/'Contents/Info.plist').read_bytes())['CFBundleExecutable']=='VoicePort'
 for binary in [helper,app/'Contents/MacOS/VoicePort']:
  assert subprocess.check_output(['lipo','-archs',str(binary)],text=True).strip()=='arm64','unexpected architecture'
 assert (app/'Contents/Resources/THIRD_PARTY_NOTICES.md').read_bytes()==(ROOT/'THIRD_PARTY_NOTICES.md').read_bytes()
 check(app/'Contents/Resources/third-party')
 subprocess.run(['codesign','--verify','--strict',str(helper)],check=True)
 subprocess.run(['codesign','--verify','--deep','--strict',str(app)],check=True)
 with tempfile.TemporaryDirectory(prefix='vp012-helper-proof-') as t:
  copied=pathlib.Path(t)/'voiceport-rs';copied.write_bytes(pathlib.Path(target).read_bytes());copied.chmod(0o755)
  subprocess.run(['codesign','--force','--sign','-',str(copied)],check=True,capture_output=True)
  assert sha(copied)==sha(helper),'helper does not match this cargo build plus ad-hoc signature'
 with tempfile.TemporaryDirectory(prefix='vp012-main-proof-') as t:
  twin=pathlib.Path(t)/'VoicePort.app';shutil.copytree(app,twin)
  shutil.copyfile(swift,twin/'Contents/MacOS/VoicePort');(twin/'Contents/MacOS/VoicePort').chmod(0o755)
  subprocess.run(['codesign','--force','--sign','-',str(twin)],check=True,capture_output=True)
  assert sha(twin/'Contents/MacOS/VoicePort')==sha(main),'main does not match this Swift VoicePort build plus bundle signature'
 return {'main_sha256':sha(main),'swift_product_sha256':sha(pathlib.Path(swift)),'main_signed_copy_matches':True,'helper_sha256':sha(helper),'cargo_product_sha256':sha(pathlib.Path(target)),'signed_copy_matches':True,'license_packages':len(check(app/'Contents/Resources/third-party')['packages'])}
if __name__=='__main__':
 import argparse
 p=argparse.ArgumentParser();p.add_argument('app',type=pathlib.Path);p.add_argument('target',type=pathlib.Path);p.add_argument('swift',type=pathlib.Path);a=p.parse_args();print(json.dumps(validate(a.app,a.target,a.swift)))
