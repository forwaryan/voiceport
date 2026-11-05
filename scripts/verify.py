#!/usr/bin/env python3
"""Full local offline development-package verification (optional safe menu smoke)."""
import argparse,datetime,hashlib,json,os,pathlib,platform,subprocess,sys,tempfile,uuid
ROOT=pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'scripts'))
from product_inventory import inventory

def safe_inventory():
 try:return inventory(ROOT)
 except (OSError,ValueError) as e:return {'sha256':None,'error':type(e).__name__}

def main():
 parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--app-smoke',action='store_true');a=parser.parse_args()
 evidence=ROOT/'.voiceport/evidence/vp012-revision-01'/('verify-'+uuid.uuid4().hex);evidence.mkdir(parents=True)
 before=safe_inventory();(evidence/'inputs-before.json').write_text(json.dumps(before,indent=2))
 report={'started_at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'system':platform.platform(),'checks':[],'source_sha256':before['sha256'],'evidence':str(evidence.relative_to(ROOT))}
 success=not before.get("error") and not before.get("missing")
 with tempfile.TemporaryDirectory(prefix='vp012-') as temp:
  temp=pathlib.Path(temp).resolve();home=temp/'synthetic-codex';home.mkdir();(home/'auth.json').write_text('synthetic invalid JSON, not credentials')
  env=dict(os.environ,CARGO_NET_OFFLINE='true',CODEX_HOME=str(home),TMPDIR=str(temp),PYTHONDONTWRITEBYTECODE='1')
  differences={k:env[k] for k in ['CARGO_NET_OFFLINE','CODEX_HOME','TMPDIR','PYTHONDONTWRITEBYTECODE']};report['environment_overrides']=differences
  def run(cmd,expected=0,marker=None,timeout=600,cwd=ROOT,input=None):
   nonlocal success
   n=len(report['checks'])+1
   try:
    r=subprocess.run(list(map(str,cmd)),cwd=cwd,env=env,input=input,capture_output=True,timeout=timeout);out,err,code=r.stdout,r.stderr,r.returncode
   except subprocess.TimeoutExpired as e:out,err,code=e.stdout or b'',e.stderr or b'',124
   except OSError as e:out,err,code=b'',str(e).encode(),127
   stdout=evidence/f'{n:02d}.stdout.log';stderr=evidence/f'{n:02d}.stderr.log';stdout.write_bytes(out);stderr.write_bytes(err)
   passed=code==expected and (marker is None or marker in (out+err).decode(errors='replace'))
   report['checks'].append({'argv':list(map(str,cmd)),'command':list(map(str,cmd)),'cwd':str(cwd),'environment_overrides':differences,'timeout_seconds':timeout,'expected_exit_code':expected,'exit_code':code,'required_output':marker,'passed':passed,'stdout':str(stdout.relative_to(ROOT)),'stderr':str(stderr.relative_to(ROOT)),'stdout_sha256':hashlib.sha256(out).hexdigest(),'stderr_sha256':hashlib.sha256(err).hexdigest()});print(f'{n:02d} exit={code} expected={expected} {cmd}',flush=True)
   success=success and passed
   return out if passed else None
  checks=[(['cargo','test','--workspace','--all-targets','--locked'],0,'0 failed'),(['cargo','fmt','--all','--','--check'],0,None),(['cargo','clippy','--workspace','--all-targets','--locked','--','-D','warnings'],0,None),(['bash',str(ROOT/'scripts/package_app.sh')],0,'Built '),(['bash','scripts/test_core.sh'],0,'0 failures'),(['bash','scripts/test_app.sh'],0,'0 failures'),(['python3','-I','-m','unittest','discover','-s','Tests/LoopTests','-v'],0,'OK'),(['python3','-I','-m','unittest','discover','-s','Tests/PackagingTests','-v'],0,'OK')]
  for cmd,code,marker in checks:
   if not success:break
   if run(cmd,code,marker,cwd=temp if 'package_app.sh' in str(cmd[-1]) else ROOT) is None:break
  try:
   if success:
    metadata=run(['cargo','metadata','--offline','--locked','--no-deps','--format-version','1']);target=pathlib.Path(json.loads(metadata)['target_directory'])
    swift=run(['swift','build','-c','release','--show-bin-path']);swift=pathlib.Path(swift.decode().strip());rust=target/'release/voiceport-rs'
    for binary in [swift/'voiceport-cli',rust]:
     isrust=binary==rust
     cases=[(['--help'],0,'transcribe'),(['demo'],0,'离线模拟'),(['doctor']+(['--auth',str(home/'auth.json')] if isrust else []),0,'Codex 登录文件'),(['bad-command'],2 if isrust else 1,'参数无效'),(['transcribe'],2 if isrust else 1,'参数无效'),(['transcribe',str(temp/'missing.m4a'),'--auth',str(home/'auth.json')],1,'登录文件格式无效')]
     for args,code,marker in cases:run([binary]+args,code,marker,timeout=10)
    run(['python3','scripts/check_bundle.py',ROOT/'dist/VoicePort.app',rust,swift/'VoicePort'],marker='signed_copy_matches')
    helper=ROOT/'dist/VoicePort.app/Contents/Helpers/voiceport-rs'
    wire=b'{"version":1,"action":"status","request_id":"q"}\n{"version":1,"action":"shutdown","request_id":"end"}\n'
    out=run([helper,'host'],marker='shutdown',input=wire,timeout=10);(evidence/'helper.stdin.jsonl').write_bytes(wire)
    if out is not None:
     success=success and [json.loads(l)['event'] for l in out.splitlines()]==['ready','status','shutdown']
    report['artifacts']={str(p.relative_to(ROOT)) if p.is_relative_to(ROOT) else str(p):{'bytes':p.stat().st_size,'sha256':hashlib.sha256(p.read_bytes()).hexdigest(),'mode':p.stat().st_mode&0o777} for p in [rust,helper,swift/'VoicePort',swift/'voiceport-cli',ROOT/'dist/VoicePort.app/Contents/MacOS/VoicePort']}
    if a.app_smoke:run([ROOT/'dist/VoicePort.app/Contents/MacOS/VoicePort','--smoke-test'],marker='menu ready',timeout=20)
  except Exception as error:
   success=False;report['verification_error']={'type':type(error).__name__,'detail':str(error)}
  report['normal_package_includes']=['swift build -c release','cargo build --workspace --release --locked'];report['unverified']=['real auth/cloud/microphone/target reception','Windows','active GUI exit','notarization/external distribution/legal approval']
 after=safe_inventory();(evidence/'inputs-after.json').write_text(json.dumps(after,indent=2));report['source_sha256_at_finish']=after['sha256'];report['inputs_unchanged']=before==after;report['passed']=success and before==after;report['finished_at']=datetime.datetime.now(datetime.timezone.utc).isoformat();(evidence/'report.json').write_text(json.dumps(report,ensure_ascii=False,indent=2));print('Evidence:',evidence/'report.json',flush=True);return 0 if report['passed'] else 1
if __name__=='__main__':sys.exit(main())
