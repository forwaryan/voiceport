#!/usr/bin/python3
import sys,json,time,os,signal
mode=os.environ.get('VP011_MODE','ok');failed=False
sequence=json.loads(os.environ.get('VP011_SESSION_SEQUENCE','[]'));job=0;sid='s1'
if mode=='no-ready':time.sleep(20)
if mode=='stubborn':signal.signal(signal.SIGTERM,signal.SIG_IGN)
def emit(e):
 b=(json.dumps(dict(version=1,**e),ensure_ascii=False)+'\n').encode()
 for i in range(0,len(b),3):sys.stdout.buffer.write(b[i:i+3]);sys.stdout.buffer.flush()
emit(dict(event='ready',state='idle'))
if mode=='no-read':time.sleep(20)
if mode=='exit':sys.exit(0)
for line in sys.stdin.buffer:
 c=json.loads(line);a=c['action'];r=c['request_id']
 if a=='status':emit(dict(event='status',state='idle',request_id=r));continue
 if a=='cancel':emit(dict(event='cancelled',state='idle',request_id=r,session_id=sid));continue
 if a=='shutdown':emit(dict(event='shutdown',state='idle',request_id=r));break
 job+=1;sid=sequence[min(job-1,len(sequence)-1)] if sequence else 's'+str(job)
 retry=a=='retry';emit(dict(event='accepted',state='transcribing',request_id=r,session_id=sid,is_retry=retry))
 if mode in ['slow','stubborn']:time.sleep(20)
 if mode=='tail':sys.stdout.buffer.write(b'{"text":"'+b'x'*67104);sys.stdout.buffer.flush();break
 if mode=='large':sys.stdout.buffer.write(b'x'*2000001+b'\n');sys.stdout.buffer.flush();break
 if mode=='bad':sys.stdout.write('{"version":1,"event":"result","state":"ready","request_id":"wrong","session_id":"s1","is_retry":false,"text":"must not deliver"}\n');sys.stdout.flush();continue
 if mode=='retry' and not retry:emit(dict(event='error',state='failed',request_id=r,session_id=sid,is_retry=False,code='Network'));continue
 emit(dict(event='result',state='ready',request_id=r,session_id=sid,is_retry=retry,text='原始\n文字\t\0'))
