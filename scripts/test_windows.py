#!/usr/bin/env python3
"""Portable progress and optional actual Windows adapter typecheck, not runtime acceptance."""
import argparse,json,pathlib,subprocess,os,tempfile,uuid,hashlib
ROOT=pathlib.Path(__file__).resolve().parents[1]
def main():
 p=argparse.ArgumentParser();p.add_argument('--adapter-typecheck',action='store_true');a=p.parse_args()
 scope=ROOT/'.voiceport/evidence/vp013-developer-02'/('checks-'+uuid.uuid4().hex);scope.mkdir(parents=True)
 target=ROOT/'.build'/('vp013-final-'+uuid.uuid4().hex);target.mkdir(parents=True)
 app=scope/'VoicePort.app'
 env=dict(os.environ,CARGO_TARGET_DIR=str(target),CARGO_NET_OFFLINE='true',CODEX_HOME=str(scope/'absent-codex'),TMPDIR=str(scope.resolve()),PYTHONDONTWRITEBYTECODE='1',VOICEPORT_APP_DIRECTORY=str(app));rows=[]
 def run(argv,expected=0):
  n=len(rows)+1
  try:r=subprocess.run(list(map(str,argv)),cwd=ROOT,env=env,capture_output=True,timeout=600);code,out,err=r.returncode,r.stdout,r.stderr
  except subprocess.TimeoutExpired as ex:code,out,err=124,ex.stdout or b'',ex.stderr or b''
  (scope/f'{n:02d}.stdout.log').write_bytes(out);(scope/f'{n:02d}.stderr.log').write_bytes(err)
  rows.append({'argv':list(map(str,argv)),'cwd':str(ROOT),'environment_overrides':{k:env[k] for k in ['CARGO_TARGET_DIR','CARGO_NET_OFFLINE','CODEX_HOME','TMPDIR','PYTHONDONTWRITEBYTECODE','VOICEPORT_APP_DIRECTORY']},'timeout_seconds':600,'exit_code':code,'expected_exit_code':expected,'stdout':f'{n:02d}.stdout.log','stderr':f'{n:02d}.stderr.log','stdout_sha256':hashlib.sha256(out).hexdigest(),'stderr_sha256':hashlib.sha256(err).hexdigest()});print(n,code,argv,flush=True)
  (scope/'report.json').write_text(json.dumps({'checks':rows,'passed':all(x['exit_code']==x['expected_exit_code'] for x in rows),'scope':'partial portable progress; native adapter-only typecheck is not full Windows build/runtime'},indent=2));return code==expected
 commands=[['cargo','test','--workspace','--all-targets','--locked'],['cargo','fmt','--all','--','--check'],['cargo','clippy','--workspace','--all-targets','--locked','--','-D','warnings']]
 if a.adapter_typecheck:commands.append(['cargo','check','-p','voiceport-windows','--no-default-features','--target','x86_64-pc-windows-gnu','--locked'])
 commands += [['bash','scripts/package_app.sh'],['python3','-I','-m','unittest','discover','-s','Tests/LoopTests','-v'],['python3','-I','-m','unittest','discover','-s','Tests/PackagingTests','-v'],['cargo','run','--locked','-p','voiceport-windows','--','--help'],['cargo','run','--locked','-p','voiceport-windows','--','--smoke-test'],['cargo','run','--locked','-p','voiceport-windows','--example','synthetic_wav','--',str(scope/'synthetic.wav')],['python3','-I','Tests/WindowsTests/read_wav.py',str(scope/'synthetic.wav')],[str(app/'Contents/MacOS/VoicePort'),'--smoke-test']]
 for c in commands:
  if not run(c):print('Evidence:',scope);return 1
 if not run(['cargo','run','--locked','-p','voiceport-windows','--','invalid-command'],2):return 1
 print('Evidence:',scope);return 0
if __name__=='__main__':raise SystemExit(main())
