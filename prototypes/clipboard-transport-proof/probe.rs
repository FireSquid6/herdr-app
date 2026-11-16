//! Standalone CEF 156 owned-fixture experiment. No production/OS clipboard claim.
mod arguments;
mod scenarios;
use cef::{rc::Rc as _, *};
use std::{collections::HashMap, io::{Read, Write}, net::TcpListener,
 sync::{Arc, Mutex, atomic::{AtomicBool, AtomicU64, Ordering}}, time::{Duration, Instant}};
const CALL: &str = "owned.transport.call";
const REPLY: &str = "owned.transport.reply";
// Fixed proof-only fixture evaluator: no page binding, website API replacement,
// string interpolation, permission grant, or production-authoritative DOM read.
const FUNCTION: &str = r#"(function(op,text,mode){
 let e=document.getElementById('editor');
 if(op==='init'){
  if(!e){e=document.createElement('textarea');e.id='editor';document.body.append(e);}
  if(!document.getElementById('events')){let l=document.createElement('pre');l.id='events';document.body.append(l);}
  if(!e.dataset.installed){e.dataset.installed='1';
   for(const type of ['copy','cut','paste','beforeinput','input']) e.addEventListener(type,event=>{
    let row={type,trusted:event.isTrusted,inputType:event.inputType??null,data:event.clipboardData?.getData('text/plain')??null};
    if(['copy','cut'].includes(type)&&e.dataset.mode==='custom'){
     event.clipboardData.setData('text/plain','CUSTOM 🦀\n exact');event.preventDefault();
    }
    if(['copy','cut','paste'].includes(type)&&e.dataset.mode==='cancel')event.preventDefault();
    row.prevented=event.defaultPrevented;
    document.getElementById('events').textContent+=JSON.stringify(row)+'\n';
   });
  }
 }
 if(op==='set'){e.value=text;e.dataset.mode=mode;e.focus();e.select();document.getElementById('events').textContent='';}
 if(op==='noselect'){e.setSelectionRange(0,0);}
 if(op==='write'){
  try{navigator.clipboard.writeText(text).then(()=>{
   document.getElementById('events').textContent+=JSON.stringify({type:'write',result:'ALLOWED'})+'\n';
  },()=>{document.getElementById('events').textContent+=JSON.stringify({type:'write',result:'DENIED'})+'\n';});}
  catch(_){document.getElementById('events').textContent+=JSON.stringify({type:'write',result:'DENIED'})+'\n';}
 }
 if(op==='deny'){
  try{navigator.clipboard.readText().then(value=>{
   document.getElementById('events').textContent+=JSON.stringify({type:'read',result:'ALLOWED',value})+'\n';
  },()=>{document.getElementById('events').textContent+=JSON.stringify({type:'read',result:'DENIED'})+'\n';});}
  catch(_){document.getElementById('events').textContent+=JSON.stringify({type:'read',result:'DENIED'})+'\n';}
 }
 return JSON.stringify({value:e.value,events:document.getElementById('events').textContent,focus:document.hasFocus(),activation:navigator.userActivation.isActive,selection:[e.selectionStart,e.selectionEnd]});
})"#;
#[derive(Default)]
struct Shared {
 ready:AtomicBool, loaded:AtomicBool, closed:AtomicBool,
 replies:Mutex<HashMap<u64,String>>, pending:Mutex<Option<(u64,i32,String)>>,
}
wrap_browser_process_handler! { struct Process { shared:Arc<Shared> } impl BrowserProcessHandler {
 fn on_context_initialized(&self){self.shared.ready.store(true,Ordering::Release);}
}}
wrap_request_context_handler! { struct ContextReady { ready:Arc<AtomicBool> } impl RequestContextHandler {
 fn on_request_context_initialized(&self,_context:Option<&mut RequestContext>){self.ready.store(true,Ordering::Release);}
}}
wrap_render_process_handler! { struct Renderer {} impl RenderProcessHandler {
 fn on_process_message_received(&self,browser:Option<&mut Browser>,frame:Option<&mut Frame>,source:ProcessId,message:Option<&mut ProcessMessage>)->i32{
  if source!=ProcessId::BROWSER{return 0;}
  let (Some(browser),Some(frame),Some(message))=(browser,frame,message)else{return 0;};
  if CefString::from(&message.name()).to_string()!=CALL{return 0;}
  let Some(args)=message.argument_list()else{return 1;};
  if args.size()!=6 || args.get_type(0)!=ValueType::INT || (1..6).any(|i|args.get_type(i)!=ValueType::STRING){return 1;}
  // Bind the command to the actual callback browser and frame, not a supplied pane.
  if frame.is_valid()!=1 || frame.is_main()!=1 || args.int(0)!=browser.identifier() || CefString::from(&args.string(1)).to_string()!=CefString::from(&frame.identifier()).to_string(){return 1;}
  let nonce=CefString::from(&args.string(2)).to_string();
  if nonce.len()>20 || nonce.parse::<u64>().ok().filter(|v|*v>0).is_none(){return 1;}
  let op=CefString::from(&args.string(3)).to_string();
  let text=CefString::from(&args.string(4)).to_string();
  let mode=CefString::from(&args.string(5)).to_string();
  if text.len()>1024*1024 || !matches!(mode.as_str(),""|"default"|"custom"|"cancel"){return 1;}
  let output=if matches!(op.as_str(),"copy"|"cut"|"paste"){
   // This is the tested protocol: PUBLIC FRAME API IN THE RENDERER, then ACK.
   match op.as_str(){"copy"=>frame.copy(),"cut"=>frame.cut(),"paste"=>frame.paste(),_=>unreachable!()}
   "{\"renderer_return_ack\":true}".to_string()
  }else{
   if !matches!(op.as_str(),"init"|"set"|"snapshot"|"noselect"|"deny"|"write"){return 1;}
   let Some(context)=frame.v8_context()else{return 1;};
   if context.enter()!=1{return 1;}
   let mut result=None;let mut exception=None;
   let ok=context.eval(Some(&FUNCTION.into()),Some(&"owned-transport-proof".into()),1,Some(&mut result),Some(&mut exception));
   let output=if ok==1{
    result.and_then(|f|f.execute_function(None,Some(&[
     v8_value_create_string(Some(&op.as_str().into())),v8_value_create_string(Some(&text.as_str().into())),v8_value_create_string(Some(&mode.as_str().into()))
    ]))).map(|v|CefString::from(&v.string_value()).to_string()).unwrap_or_else(||"CALL_FAILED".into())
   }else{"EVAL_FAILED".into()};context.exit();output
  };
  let Some(mut reply)=process_message_create(Some(&REPLY.into()))else{return 1;};
  let Some(args)=reply.argument_list()else{return 1;};
  args.set_string(0,Some(&nonce.as_str().into()));args.set_string(1,Some(&output.as_str().into()));
  frame.send_process_message(ProcessId::BROWSER,Some(&mut reply));1
 }
}}
wrap_app! { struct Application { shared:Arc<Shared> } impl App {
 fn browser_process_handler(&self)->Option<BrowserProcessHandler>{Some(Process::new(self.shared.clone()))}
 fn render_process_handler(&self)->Option<RenderProcessHandler>{Some(Renderer::new())}
 fn on_before_command_line_processing(&self,_kind:Option<&CefString>,command:Option<&mut CommandLine>){if let Some(command)=command{
  command.append_switch_with_value(Some(&"ozone-platform".into()),Some(&"headless".into()));
  for key in ["disable-gpu","disable-background-networking","no-first-run"]{command.append_switch(Some(&key.into()));}
 }}
}}
wrap_render_handler! { struct Pixels {} impl RenderHandler {
 fn view_rect(&self,_browser:Option<&mut Browser>,rect:Option<&mut Rect>){if let Some(rect)=rect{*rect=Rect{x:0,y:0,width:640,height:480};}}
 fn on_paint(&self,_browser:Option<&mut Browser>,_kind:PaintElementType,_dirty:Option<&[Rect]>,_buffer:*const u8,_width:i32,_height:i32){}
}}
wrap_load_handler! { struct Loaded { shared:Arc<Shared> } impl LoadHandler {
 fn on_load_end(&self,_browser:Option<&mut Browser>,frame:Option<&mut Frame>,_code:i32){if frame.is_some_and(|f|f.is_main()==1){self.shared.loaded.store(true,Ordering::Release);}}
}}
wrap_life_span_handler! { struct Life { shared:Arc<Shared> } impl LifeSpanHandler {
 fn on_before_close(&self,_browser:Option<&mut Browser>){if !self.shared.closed.swap(true,Ordering::AcqRel){LIVE_PAGES.fetch_sub(1,Ordering::AcqRel);}}
 fn on_before_popup(&self,_browser:Option<&mut Browser>,_frame:Option<&mut Frame>,_id:i32,_url:Option<&CefString>,_name:Option<&CefString>,_disposition:WindowOpenDisposition,_gesture:i32,_features:Option<&PopupFeatures>,_info:Option<&mut WindowInfo>,_client:Option<&mut Option<Client>>,_settings:Option<&mut BrowserSettings>,_extra:Option<&mut Option<DictionaryValue>>,_access:Option<&mut i32>)->i32{1}
}}
wrap_permission_handler! { struct Permissions {} impl PermissionHandler {
 fn on_show_permission_prompt(&self,_browser:Option<&mut Browser>,_id:u64,_origin:Option<&CefString>,requested:u32,callback:Option<&mut PermissionPromptCallback>)->i32{eprintln!("DENY permission mask {requested}");if let Some(callback)=callback{callback.cont(PermissionRequestResult::DENY);}1}
 fn on_request_media_access_permission(&self,_browser:Option<&mut Browser>,_frame:Option<&mut Frame>,_origin:Option<&CefString>,_requested:u32,callback:Option<&mut MediaAccessCallback>)->i32{if let Some(callback)=callback{callback.cont(0);}1}
}}
wrap_resource_request_handler! { struct Resource { url:String } impl ResourceRequestHandler {
 fn on_before_resource_load(&self,_browser:Option<&mut Browser>,_frame:Option<&mut Frame>,request:Option<&mut Request>,_callback:Option<&mut Callback>)->ReturnValue{if request.is_some_and(|r|CefString::from(&r.url()).to_string()==self.url){ReturnValue::CONTINUE}else{ReturnValue::CANCEL}}
 fn on_protocol_execution(&self,_browser:Option<&mut Browser>,_frame:Option<&mut Frame>,_request:Option<&mut Request>,allow:Option<&mut i32>){if let Some(allow)=allow{*allow=0;}}
}}
wrap_request_handler! { struct Requests { url:String } impl RequestHandler {
 fn on_before_browse(&self,_browser:Option<&mut Browser>,frame:Option<&mut Frame>,request:Option<&mut Request>,_gesture:i32,_redirect:i32)->i32{i32::from(!frame.is_some_and(|f|f.is_main()==1)||!request.is_some_and(|r|CefString::from(&r.url()).to_string()==self.url))}
 fn resource_request_handler(&self,_browser:Option<&mut Browser>,_frame:Option<&mut Frame>,_request:Option<&mut Request>,_nav:i32,_download:i32,_origin:Option<&CefString>,_disable:Option<&mut i32>)->Option<ResourceRequestHandler>{Some(Resource::new(self.url.clone()))}
}}
wrap_client! { struct ClientImpl { shared:Arc<Shared>, url:String } impl Client {
 fn render_handler(&self)->Option<RenderHandler>{Some(Pixels::new())}
 fn load_handler(&self)->Option<LoadHandler>{Some(Loaded::new(self.shared.clone()))}
 fn life_span_handler(&self)->Option<LifeSpanHandler>{Some(Life::new(self.shared.clone()))}
 fn permission_handler(&self)->Option<PermissionHandler>{Some(Permissions::new())}
 fn request_handler(&self)->Option<RequestHandler>{Some(Requests::new(self.url.clone()))}
 fn on_process_message_received(&self,browser:Option<&mut Browser>,frame:Option<&mut Frame>,source:ProcessId,message:Option<&mut ProcessMessage>)->i32{
  if source!=ProcessId::RENDERER{return 0;}
  let(Some(browser),Some(frame),Some(message))=(browser,frame,message)else{return 0;};
  if CefString::from(&message.name()).to_string()!=REPLY{return 0;}
  let Some(args)=message.argument_list()else{return 1;};
  if args.size()!=2 || (0..2).any(|i|args.get_type(i)!=ValueType::STRING){return 1;}
  let Ok(nonce)=CefString::from(&args.string(0)).to_string().parse::<u64>()else{return 1;};
  let owner=(nonce,browser.identifier(),CefString::from(&frame.identifier()).to_string());
  if frame.is_valid()!=1 || frame.is_main()!=1 || self.shared.pending.lock().unwrap().as_ref()!=Some(&owner){return 1;}
  let text=CefString::from(&args.string(1)).to_string();if text.len()>1024*1024{return 1;}
  self.shared.replies.lock().unwrap().insert(nonce,text);1
 }
}}
fn pump_until(mut check:impl FnMut()->bool,limit:Duration)->Result<(),String>{
 let start=Instant::now();while !check(){if start.elapsed()>limit{return Err("bounded wait expired".into());}do_message_loop_work();std::thread::sleep(Duration::from_millis(3));}Ok(())
}
static NONCE:AtomicU64=AtomicU64::new(0);
static LIVE_PAGES:AtomicU64=AtomicU64::new(0);
struct Page {browser:Browser,shared:Arc<Shared>,_context:RequestContext}
impl Page {
 fn new(url:&str)->Result<Self,String>{
  let ready=Arc::new(AtomicBool::new(false));
  let mut context=request_context_create_context(Some(&RequestContextSettings::default()),Some(&mut ContextReady::new(ready.clone()))).ok_or("context creation failed")?;
  pump_until(||ready.load(Ordering::Acquire),Duration::from_secs(10))?;
  if context.is_global()!=0{return Err("global context forbidden".into());}
  context.set_content_setting(None,None,ContentSettingTypes::CLIPBOARD_READ_WRITE,ContentSettingValues::BLOCK);
  let shared=Arc::new(Shared::default());let mut client=ClientImpl::new(shared.clone(),url.into());
  let browser=browser_host_create_browser_sync(Some(&WindowInfo::default().set_as_windowless(0)),Some(&mut client),Some(&url.into()),Some(&BrowserSettings{windowless_frame_rate:1,..Default::default()}),None,Some(&mut context)).ok_or("browser creation failed")?;
  // Register ownership immediately: on init/load failure Drop still closes it.
  LIVE_PAGES.fetch_add(1,Ordering::AcqRel);
  let page=Self{browser,shared,_context:context};
  pump_until(||page.shared.loaded.load(Ordering::Acquire),Duration::from_secs(15))?;
  page.call("init","","")?;Ok(page)
 }
 fn focus(&self,yes:bool){self.browser.host().unwrap().set_focus(i32::from(yes));}
 fn call(&self,op:&str,text:&str,mode:&str)->Result<serde_json::Value,String>{
  let nonce=NONCE.fetch_add(1,Ordering::AcqRel)+1;
  let frame=self.browser.main_frame().ok_or("missing frame")?;
  let frame_id=CefString::from(&frame.identifier()).to_string();
  *self.shared.pending.lock().unwrap()=Some((nonce,self.browser.identifier(),frame_id.clone()));
  let mut message=process_message_create(Some(&CALL.into())).ok_or("message creation failed")?;
  let args=message.argument_list().unwrap();args.set_int(0,self.browser.identifier());
  for (i,text) in [frame_id.as_str(),nonce.to_string().as_str(),op,text,mode].into_iter().enumerate(){args.set_string(i+1,Some(&text.into()));}
  frame.send_process_message(ProcessId::RENDERER,Some(&mut message));
  let wait=pump_until(||self.shared.replies.lock().unwrap().contains_key(&nonce),Duration::from_secs(3));
  *self.shared.pending.lock().unwrap()=None;wait?;
  let output=self.shared.replies.lock().unwrap().remove(&nonce).unwrap();
  serde_json::from_str(&output).map_err(|e|format!("bad reply {e}: {output}"))
 }
 fn native(&self,op:&str)->Result<(),String>{
  let ack=self.call(op,"","")?;if ack["renderer_return_ack"]!=true{return Err("missing renderer return ACK".into());}Ok(())
 }
 fn set(&self,text:&str,mode:&str)->Result<(),String>{self.focus(true);self.call("set",text,mode)?;Ok(())}
 fn snapshot(&self)->Result<serde_json::Value,String>{self.call("snapshot","","")}
 fn wait(&self,check:impl Fn(&serde_json::Value)->bool)->Result<serde_json::Value,String>{
  let start=Instant::now();loop{let data=self.snapshot()?;if check(&data){return Ok(data);}if start.elapsed()>Duration::from_secs(2){return Err(format!("event wait expired {data}"));}}
 }
 fn close(&self)->Result<(),String>{
  self.shared.pending.lock().unwrap().take();self.browser.host().unwrap().close_browser(1);
  pump_until(||self.shared.closed.load(Ordering::Acquire),Duration::from_secs(5))
 }
}
impl Drop for Page {fn drop(&mut self){if !self.shared.closed.load(Ordering::Acquire){let _=self.close();}}}
fn events(data:&serde_json::Value)->Vec<serde_json::Value>{data["events"].as_str().unwrap().lines().map(|line|serde_json::from_str(line).unwrap()).collect()}
fn has_event(data:&serde_json::Value,event:&str)->bool{events(data).iter().any(|e|e["type"]==event&&e["trusted"]==true)}
fn clipboard_read(shuttle:&Page)->Result<serde_json::Value,String>{
 shuttle.set("","default")?;shuttle.browser.main_frame().unwrap().paste();shuttle.wait(|d|has_event(d,"paste"))
}
fn seed(shuttle:&Page,text:&str)->Result<(),String>{
 shuttle.set(text,"default")?;shuttle.browser.main_frame().unwrap().copy();
 shuttle.wait(|d|has_event(d,"copy"))?;
 // Fixture seeding may poll to establish test precondition, NOT source completion.
 let start=Instant::now();loop{let actual=clipboard_read(shuttle)?;if actual["value"]==text{return Ok(());}if start.elapsed()>Duration::from_secs(2){return Err(format!("seed verification expired: {actual}"));}}
}
fn run()->Result<(),String>{
 let listener=TcpListener::bind("127.0.0.1:0").map_err(|e|e.to_string())?;
 let address=listener.local_addr().unwrap();
 std::thread::spawn(move||{for stream in listener.incoming(){let Ok(mut stream)=stream else{break;};
  let _=stream.set_read_timeout(Some(Duration::from_secs(2)));let mut request=[0;4096];let _=stream.read(&mut request);
  let body="<!doctype html><meta charset=utf-8><title>Owned transport proof</title><textarea id=editor></textarea><pre id=events></pre>";
  let _=write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Security-Policy: default-src 'none'; script-src 'none'; frame-src 'none'; connect-src 'none'\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body);
 }});
 let a=Page::new(&format!("http://{address}/source-a"))?;
 let b=Page::new(&format!("http://{address}/source-b"))?;
 let shuttle=Page::new("about:blank")?;
 for (left,right) in [(&a,&b),(&a,&shuttle),(&b,&shuttle)]{let mut c=right._context.clone();if left._context.is_sharing_with(Some(&mut c))!=0{return Err("contexts share".into());}}
 shuttle.browser.host().unwrap().was_hidden(1);
 println!("PROOF three separate non-global ephemeral contexts; CLIPBOARD_READ_WRITE BLOCK; prompt DENY; shuttle hidden");
 let outcome=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||scenarios::run(&a,&b,&shuttle)));
 let mut clean=true;for page in [&a,&b,&shuttle]{if let Err(e)=page.close(){eprintln!("CLOSE ERROR {e}");clean=false;}}
 println!("PROOF cleanup all_on_before_close={clean}");
 if !clean{return Err("close timeout".into());}
 match outcome{Ok(result)=>result,Err(_)=>Err("scenario assertion panic (closed all browsers)".into())}
}
fn main(){
 let argv:Vec<String>=std::env::args().collect();
 let invocation=arguments::classify_argv(&argv).expect("sandbox/helper argv rejected");
 assert!(!std::env::var_os("CHROME_DEVEL_SANDBOX").is_some_and(|value|value.is_empty()));
 assert!(!std::fs::read_to_string("/proc/self/status").unwrap().lines().any(|l|l.starts_with("Uid:\t0\t")));
 let shared=Arc::new(Shared::default());let mut app=Application::new(shared.clone());
 let _=api_hash(cef::sys::CEF_API_VERSION_LAST,0);let args=cef::args::Args::new();
 if invocation==arguments::Invocation::Helper{let code=execute_process(Some(args.as_main_args()),Some(&mut app),std::ptr::null_mut());std::process::exit(if code>=0{code}else{2});}
 assert_eq!(std::env::var("NATIVE_WAYLAND_HEADLESS").as_deref(),Ok("1"));assert!(std::env::var_os("DISPLAY").is_none());
 let home=std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
 assert!(home.starts_with("/var/tmp/native-wayland-browser-")||home.parent().is_some_and(|p|p.to_string_lossy().starts_with("/var/tmp/native-wayland-browser-")));
 let runtime=std::env::var("HERDR_BROWSER_RUNTIME").unwrap();let executable=std::env::current_exe().unwrap();
 let settings=Settings{no_sandbox:0,windowless_rendering_enabled:1,external_message_pump:1,
  root_cache_path:home.join("cef-root").to_string_lossy().as_ref().into(),browser_subprocess_path:executable.to_string_lossy().as_ref().into(),resources_dir_path:runtime.as_str().into(),locales_dir_path:format!("{runtime}/locales").as_str().into(),..Default::default()};
 assert_eq!(initialize(Some(args.as_main_args()),Some(&settings),Some(&mut app),std::ptr::null_mut()),1);
 let outcome=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||{
  pump_until(||shared.ready.load(Ordering::Acquire),Duration::from_secs(10))?;run()
 }));
 let live=LIVE_PAGES.load(Ordering::Acquire);
 if live!=0{eprintln!("PROOF cef_shutdown=false still_live={live}; owned harness will reap process group");std::process::exit(1);}
 shutdown();println!("PROOF cef_shutdown=true");
 match outcome{Ok(Ok(()))=>println!("PROOF experiment_complete=true production_barrier=false"),other=>{eprintln!("PROOF experiment error {other:?}");std::process::exit(1);}}
}
