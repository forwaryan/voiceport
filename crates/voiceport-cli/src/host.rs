use std::{
    collections::HashSet,
    future::Future,
    io::{BufReader, Read, Write},
    pin::Pin,
    sync::Arc,
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use voiceport_core::{
    Error, Result,
    protocol::{self, Command, Event},
    session::{Job, Phase, Sessions},
};
pub type Service =
    Arc<dyn Fn(Job) -> Pin<Box<dyn Future<Output = Result<String>> + Send>> + Send + Sync>;
type Worker = JoinHandle<(Job, Result<String>)>;
type Output = mpsc::Sender<(Vec<u8>, oneshot::Sender<bool>)>;
async fn emit(output: &Output, event: Event, stall: Duration) -> Result<()> {
    let encoded = event.encode();
    let overflow = encoded.is_none();
    let bytes = encoded.unwrap_or_else(|| {
        Event::error(None, "output_too_large", "idle")
            .encode()
            .unwrap()
    });
    let (ack, done) = oneshot::channel();
    tokio::time::timeout(stall, async {
        output
            .send((bytes, ack))
            .await
            .map_err(|_| Error::Network)?;
        if !done.await.map_err(|_| Error::Network)? {
            return Err(Error::Network);
        }
        Ok(())
    })
    .await
    .map_err(|_| Error::TimedOut)??;
    if overflow {
        Err(Error::InvalidResponse)
    } else {
        Ok(())
    }
}
pub async fn run(
    input: impl Read + Send + 'static,
    mut writer: impl Write + Send + 'static,
    service: Service,
    stall: Duration,
) -> Result<()> {
    let (commands, mut incoming) = mpsc::channel(1);
    // Detached OS threads, not Tokio blocking tasks: an open stdin or blocked
    // stdout cannot keep runtime shutdown waiting. Queues stay capacity one.
    std::thread::spawn(move || {
        let mut reader = BufReader::new(input);
        while let Ok(Some(command)) = protocol::frame(&mut reader) {
            if commands.blocking_send(command).is_err() {
                break;
            }
        }
    });
    let (output, mut outgoing) = mpsc::channel::<(Vec<u8>, oneshot::Sender<bool>)>(1);
    std::thread::spawn(move || {
        while let Some((bytes, ack)) = outgoing.blocking_recv() {
            let ok = writer
                .write_all(&bytes)
                .and_then(|_| writer.flush())
                .is_ok();
            let _ = ack.send(ok);
            if !ok {
                break;
            }
        }
    });
    let mut sessions = Sessions::default();
    let mut worker: Option<Worker> = None;
    let result = serve(
        &mut incoming,
        &output,
        &service,
        stall,
        &mut sessions,
        &mut worker,
    )
    .await;
    sessions.shutdown();
    if let Some(worker) = worker {
        worker.abort();
    }
    result
}
async fn serve(
    incoming: &mut mpsc::Receiver<Option<Command>>,
    output: &Output,
    service: &Service,
    stall: Duration,
    sessions: &mut Sessions,
    worker: &mut Option<Worker>,
) -> Result<()> {
    emit(output, Event::new("ready", None, None, "idle"), stall).await?;
    let mut seen = HashSet::new();
    loop {
        tokio::select! { biased;
            _=tokio::signal::ctrl_c()=>return Err(Error::Cancelled),
            command=incoming.recv()=> {
                let Some(command)=command else{return Ok(());};
                let Some(command)=command else {emit(output,Event::error(None,"invalid_frame",sessions.phase().name()),stall).await?;continue;};
                let id=command.id().to_owned();
                if seen.contains(&id) {emit(output,Event::error(Some(id),"duplicate_request",sessions.phase().name()),stall).await?;continue;}
                if seen.len()==protocol::REQUEST_LIMIT {emit(output,Event::error(Some(id),"request_limit",sessions.phase().name()),stall).await?;return Ok(());}
                seen.insert(id.clone());
                let job=match command {
                    Command::Status{..}=>{emit(output,Event::new("status",Some(id),sessions.current().map(Job::session_id),sessions.phase().name()),stall).await?;None},
                    Command::Shutdown{..}=>{sessions.shutdown();if let Some(w)=worker.take(){w.abort();}emit(output,Event::new("shutdown",Some(id),None,"idle"),stall).await?;return Ok(());},
                    Command::Cancel{session_id,..}=>{
                        if sessions.cancel(&session_id) {if let Some(w)=worker.take(){w.abort();}emit(output,Event::new("cancelled",Some(id),Some(session_id),"idle"),stall).await?;}
                        else {emit(output,Event::error(Some(id),"stale_session",sessions.phase().name()),stall).await?;}None
                    },
                    Command::Retry{session_id,..}=>{
                        let job=sessions.retry(&session_id,id.clone());if job.is_none(){emit(output,Event::error(Some(id),"not_retryable",sessions.phase().name()),stall).await?;}job
                    },
                    Command::Transcribe{audio_path,language,..}=>{
                        let old=if sessions.phase()==Phase::Transcribing {sessions.current().cloned()}else{None};
                        let job=sessions.start(id,audio_path.into(),language,false);
                        if let Some(old)=old {let mut event=Event::new("replaced",Some(old.request_id.clone()),Some(old.session_id()),"idle");event.code=Some("replaced".into());emit(output,event,stall).await?;}
                        job
                    }
                };
                if let Some(job)=job {
                    if let Some(old)=worker.take(){old.abort();}
                    let mut event=Event::new("accepted",Some(job.request_id.clone()),Some(job.session_id()),"transcribing");event.is_retry=Some(job.is_retry);emit(output,event,stall).await?;
                    let service=service.clone();*worker=Some(tokio::spawn(async move {let result=service(job.clone()).await;(job,result)}));
                }
            },
            completed=async {match worker.as_mut(){Some(w)=>w.await,None=>std::future::pending().await}}=> {
                worker.take();let (job,result)=completed.map_err(|_|Error::Network)?;
                if sessions.complete(job.generation,&result) {
                    let mut event=Event::new(if result.is_ok(){"result"}else{"error"},Some(job.request_id.clone()),Some(job.session_id()),sessions.phase().name());event.is_retry=Some(job.is_retry);
                    match result {Ok(text)=>event.text=Some(text),Err(error)=>event.code=Some(format!("{error:?}"))};emit(output,event,stall).await?;
                }
            }
        }
    }
}
