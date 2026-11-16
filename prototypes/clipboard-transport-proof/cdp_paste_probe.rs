//! New bounded stock-CEF Input.dispatchKeyEvent experiment, loaded as the
//! scenarios module of a CACHE-generated adapter of the unchanged legacy probe.
//! No production authority, page bindings, OS clipboard or HTTP debug endpoint.
use super::*;
use std::sync::OnceLock;
use serde_json::{Value, json};
static ACKS: OnceLock<Mutex<HashMap<i32,(bool,String,Instant)>>> = OnceLock::new();
static REPORTS: OnceLock<Mutex<Vec<(Instant,String)>>> = OnceLock::new();
static BLOCKS: AtomicU64 = AtomicU64::new(0);
fn acks()->&'static Mutex<HashMap<i32,(bool,String,Instant)>> { ACKS.get_or_init(Mutex::default) }
fn reports()->&'static Mutex<Vec<(Instant,String)>> { REPORTS.get_or_init(Mutex::default) }
pub(super) fn allowed_resource(url:&str,owned:&str)->bool {
 if url==owned{return true;}
 let Some((origin,_))=owned.rsplit_once('/')else{return false;};
 owned.starts_with("http://127.0.0.1:") && ["/blocked","/unblocked","/paste"].iter().any(|p|url==format!("{origin}{p}"))
}
pub(super) fn report(request:&[u8]) {
 let line=String::from_utf8_lossy(request).lines().next().unwrap_or_default().to_owned();
 let path=line.split_whitespace().nth(1).unwrap_or_default();
 if ["/blocked","/unblocked","/paste"].contains(&path){
  reports().lock().unwrap().push((Instant::now(),path.to_owned()));
  if path=="/blocked"{BLOCKS.fetch_add(1,Ordering::AcqRel);}
 }
}
wrap_dev_tools_message_observer! { struct Observer {} impl DevToolsMessageObserver {
 fn on_dev_tools_method_result(&self,_browser:Option<&mut Browser>,id:i32,success:i32,result:Option<&[u8]>){
  acks().lock().unwrap().insert(id,(success==1,String::from_utf8_lossy(result.unwrap_or_default()).into_owned(),Instant::now()));
 }
}}
fn send(page:&Page,params:Value)->Result<i32,String>{
 let id=(NONCE.fetch_add(1,Ordering::AcqRel)+1) as i32;
 let mut params=parse_json(Some(&params.to_string().as_str().into()),JsonParserOptions::default()).and_then(|v|v.dictionary()).ok_or("bad CDP params")?;
 if page.browser.host().unwrap().execute_dev_tools_method(id,Some(&"Input.dispatchKeyEvent".into()),Some(&mut params))!=id{return Err("CDP dispatch rejected".into());}
 Ok(id)
}
fn wait_ack(id:i32)->Result<(bool,String,Instant),String>{
 pump_until(||acks().lock().unwrap().contains_key(&id),Duration::from_secs(4))?;
 Ok(acks().lock().unwrap().remove(&id).unwrap())
}
fn params(candidate:&str,up:bool)->Value {
 if candidate=="commands" {
  if up {json!({"type":"keyUp","key":"Unidentified","code":"Unidentified","windowsVirtualKeyCode":0})}
  else {json!({"type":"rawKeyDown","key":"Unidentified","code":"Unidentified","windowsVirtualKeyCode":0,"commands":["Paste"]})}
 } else {json!({"type":if up{"keyUp"}else{"rawKeyDown"},"key":"v","code":"KeyV","windowsVirtualKeyCode":86,"modifiers":2})}
}
fn custom_seed(shuttle:&Page,text:&str)->Result<(),String>{
 shuttle.set(text,"custom")?;shuttle.browser.main_frame().unwrap().copy();
 shuttle.wait(|d|has_event(d,"copy"))?;
 let start=Instant::now();loop{
  let data=clipboard_read(shuttle)?;
  let paste=events(&data).into_iter().find(|e|e["type"]=="paste").ok_or("seed paste missing")?;
  if (text.is_empty() && paste["data"]=="" && (paste["hasPlain"]==true||paste["hasFormats"]==false)) || (!text.is_empty()&&data["value"]==text){break;}
  if start.elapsed()>Duration::from_secs(2){return Err(format!("seed failed {data}"));}
 }
 shuttle.focus(false);Ok(())
}
fn block_source(a:&Page)->Result<Instant,String>{
 let before=BLOCKS.load(Ordering::Acquire);
 a.browser.host().unwrap().send_key_event(Some(&KeyEvent{type_:KeyEventType::RAWKEYDOWN,windows_key_code:119,native_key_code:119,focus_on_editable_field:1,..Default::default()}));
 pump_until(||BLOCKS.load(Ordering::Acquire)>before,Duration::from_secs(2))?;
 Ok(reports().lock().unwrap().iter().rev().find(|(_,p)|p=="/blocked").unwrap().0)
}
fn gate(page:&Page,label:&str,focus:bool)->Result<Value,String>{
 page.set("UNCHANGED","default")?;page.focus(focus);
 let before=page.snapshot()?;page.call("reads","","")?;
 let after=page.wait(|d|events(d).iter().filter(|e|e["type"]=="api_read").count()==2)?;
 let denied=events(&after).iter().filter(|e|e["type"]=="api_read"&&e["result"]=="DENIED").count()==2;
 let row=json!({"label":label,"before":before,"after":after,"both_denied":denied});
 println!("CDP_GATE {row}");Ok(row)
}
pub(super) fn run(a:&Page,b:&Page,shuttle:&Page)->Result<(),String>{
 let _registration=a.browser.host().unwrap().add_dev_tools_message_observer(Some(&mut Observer::new())).ok_or("observer failed")?;
 let mut gates=vec![gate(a,"A-focused-inactive",true)?,gate(b,"B-independent-background",false)?];
 a.set("UNCHANGED","default")?;
 let mouse=MouseEvent{x:20,y:20,modifiers:0};
 for up in [0,1]{a.browser.host().unwrap().send_mouse_click_event(Some(&mouse),MouseButtonType::LEFT,up,1);}
 a.wait(|d|d["activation"]==true)?;
 gates.push(gate(a,"A-focused-click-activated-before-paste",true)?);
 let mut rows=Vec::new();
 // Sensitivity control: the known browser-native-return race must be visible
 // with this very fixture/helper before attributing any difference to CDP ACK.
 custom_seed(shuttle,"OWNED_CONTROL_SEED")?;a.set("UNCHANGED","default")?;
 let control_block=block_source(a)?;let control_sent=Instant::now();
 a.browser.main_frame().unwrap().paste();let control_return=Instant::now();
 custom_seed(shuttle,"OWNED_CONTROL_POST_RETURN_MARKER")?;
 let control_overwrite=Instant::now();let control=a.snapshot()?;
 let control_rows=events(&control);
 let reproduced=control_rows.iter().any(|e|e["type"]=="paste"&&e["trusted"]==true&&e["data"]=="OWNED_CONTROL_POST_RETURN_MARKER")&&control["value"]=="OWNED_CONTROL_POST_RETURN_MARKER";
 println!("CDP_CONTROL {}",json!({"browser_native_return_ms":control_return.duration_since(control_sent).as_millis(),"overwrite_done_ms_from_block_report":control_overwrite.duration_since(control_block).as_millis(),"source":control,"self_interference_reproduced":reproduced}));
 for candidate in ["commands","ctrl-v","native-keyup"] {
  for mode in ["normal","cancel","empty","blocked"] {
   let seed=if mode=="empty"{""}else{" \nOWNED CDP fresh 🦀 中文\t "};
   custom_seed(shuttle,seed)?;
   a.set("UNCHANGED",if mode=="cancel"{"cancel"}else{"default"})?;
   let report_base=reports().lock().unwrap().len();
   let blocked=if mode=="blocked"{Some(block_source(a)?)}else{None};
   let sent=Instant::now();
   let id=if candidate=="native-keyup" {
    a.browser.main_frame().unwrap().paste();send(a,params("commands",true))?
   } else {send(a,params(candidate,false))?};
   let (success,result,ack)=wait_ack(id)?;
   let ack_from_block_ms=blocked.map(|t|ack.duration_since(t).as_millis());
   // CRITICAL: immediately at method-result ACK overwrite via the independent
   // helper, BEFORE source snapshots, extra CEF ACKs, keyup, or source DOM polls.
   let overwrite_started=Instant::now();
   let overwrite="OWNED_POST_ACK_INTERFERENCE";
   custom_seed(shuttle,overwrite)?;
   let overwrite_done=Instant::now();
   let source=a.snapshot()?;
   let up=if candidate=="native-keyup"{(true,"already-keyup".into(),ack)}else{wait_ack(send(a,params(candidate,true))?)?};
   let source_after_keyup=a.snapshot()?;
   let paste=events(&source).into_iter().find(|e|e["type"]=="paste");
   let exact=paste.as_ref().is_some_and(|e|e["trusted"]==true&&e["data"]==seed);
   let value_expected=if mode=="cancel"{"UNCHANGED"}else{seed};
   let value_exact=source["value"]==value_expected;
   let report_rows:Vec<Value>=reports().lock().unwrap().iter().skip(report_base).map(|(at,path)|json!({"path":path,"ms_from_dispatch":at.checked_duration_since(sent).map(|d|d.as_millis()),"before_ack":*at<=ack})).collect();
   let row=json!({"candidate":candidate,"mode":mode,"method_success":success,"method_result":result,
    "ack_ms":ack.duration_since(sent).as_millis(),"ack_ms_from_block_report":ack_from_block_ms,
    "overwrite_start_ms_after_ack":overwrite_started.duration_since(ack).as_millis(),
    "overwrite_done_ms_after_ack":overwrite_done.duration_since(ack).as_millis(),
    "trusted_paste_exact":exact,"field_exact":value_exact,"source":source,
    "keyup_success":up.0,"source_after_keyup":source_after_keyup,"reports":report_rows});
   println!("CDP_CASE {row}");rows.push(row);
  }
 }
 for candidate in ["commands","ctrl-v","native-keyup"] {
  custom_seed(shuttle,"OWNED_KEYDOWN_POLICY_SEED")?;
  a.set("UNCHANGED","block-keydown")?;
  println!("CDP_KEYDOWN_STAGE candidate={candidate} before_down");
  let down=if candidate=="native-keyup" {
   a.browser.main_frame().unwrap().paste();wait_ack(send(a,params("commands",true))?)?
  }else{wait_ack(send(a,params(candidate,false))?)?};
  println!("CDP_KEYDOWN_STAGE candidate={candidate} after_down success={}",down.0);
  let up=if candidate=="native-keyup"{(true,"already-keyup".into(),down.2)}else{wait_ack(send(a,params(candidate,true))?)?};
  println!("CDP_KEYDOWN_STAGE candidate={candidate} after_up success={}",up.0);
  let source=a.snapshot()?;
  println!("CDP_KEYDOWN_POLICY {}",json!({"candidate":candidate,"method_success":down.0,
   "keyup_success":up.0,"source":source,"production_accepted":false}));
 }
 println!("CDP_TOTAL {}",json!({"cases":rows.len(),"method_successes":rows.iter().filter(|r|r["method_success"]==true).count(),
  "trusted_exact_paste_cases":rows.iter().filter(|r|r["trusted_paste_exact"]==true).count(),
  "field_exact_cases":rows.iter().filter(|r|r["field_exact"]==true).count(),
  "gate_cases":gates.len(),"all_read_gates_denied":gates.iter().all(|g|g["both_denied"]==true),
  "production_accepted":false,"formal_barrier_claim":false,"OS_clipboard_claim":false}));
 Ok(())
}
pub(super) const FIXTURE:&str=r#"(function(op,text,mode){
 let e=document.getElementById('editor'),l=document.getElementById('events');
 if(op==='init'){
  if(!e){e=document.createElement('textarea');e.id='editor';document.body.append(e);}
  if(!l){l=document.createElement('pre');l.id='events';document.body.append(l);}
 }
 const add=row=>l.textContent+=JSON.stringify(row)+'\n';
 if(op==='init'&&!e.dataset.installed){
  e.dataset.installed='1';
  for(const type of ['keydown','keyup','copy','paste','beforeinput','input'])e.addEventListener(type,event=>{
   const dt=event.clipboardData;
   let row={type,trusted:event.isTrusted,key:event.key??null,code:event.code??null,ctrl:event.ctrlKey??null,
    inputType:event.inputType??null,data:dt?dt.getData('text/plain'):null,
    hasPlain:dt?Array.from(dt.types).includes('text/plain'):null,hasFormats:dt?dt.types.length>0:null,at:performance.now()};
   if(type==='keydown'&&e.dataset.mode==='block-keydown')event.preventDefault();
   if(type==='copy'&&e.dataset.mode==='custom'){dt.setData('text/plain',e.dataset.payload);event.preventDefault();}
   if(type==='paste'){
    if(e.dataset.mode==='cancel')event.preventDefault();
    if(location.protocol==='http:')fetch('/paste',{method:'POST',body:'owned-paste-observed'}).catch(()=>{});
   }
   row.prevented=event.defaultPrevented;add(row);
   if(type==='keydown'&&event.key==='F8'){
    fetch('/blocked',{method:'POST',body:'owned-block-start'}).catch(()=>{});
    const until=performance.now()+800;while(performance.now()<until){}
    add({type:'block_end',at:performance.now()});
    fetch('/unblocked',{method:'POST',body:'owned-block-end'}).catch(()=>{});
   }
  });
 }
 if(op==='set'){e.dataset.mode=mode;e.dataset.payload=text;e.value=mode==='custom'?'x':text;e.focus();e.select();l.textContent='';}
 if(op==='select'){e.focus();e.select();}
 if(op==='reads')for(const action of ['read','readText']){
  try{navigator.clipboard[action]().then(()=>add({type:'api_read',action,result:'ALLOWED'}),()=>add({type:'api_read',action,result:'DENIED'}));}
  catch(_){add({type:'api_read',action,result:'DENIED'});}
 }
 return JSON.stringify({value:e.value,events:l.textContent,focus:document.hasFocus(),activation:navigator.userActivation.isActive,selection:[e.selectionStart,e.selectionEnd]});
})"#;
