use super::*;
const UNICODE: &str = " \n🦀 café 中文\nlast\t ";
fn unique(prefix:&str)->String{format!("{prefix}-{}",NONCE.fetch_add(1,Ordering::AcqRel))}
fn prepare(shuttle:&Page)->Result<(),String>{shuttle.set("","default")?;shuttle.focus(false);Ok(())}
fn extract(shuttle:&Page)->Result<serde_json::Value,String>{
 // No sleeps, DOM source-event wait, retry, or cancellation settling window.
 shuttle.browser.main_frame().unwrap().paste();shuttle.wait(|d|has_event(d,"paste"))
}
fn deny(page:&Page,label:&str,focused:bool)->Result<(),String>{
 page.set("denial fixture","default")?;page.focus(focused);
 let before=page.snapshot()?;
 if before["activation"]!=false{return Err(format!("{label} unexpectedly activated {before}"));}
 if before["focus"]!=focused{return Err(format!("{label} wrong focus {before}"));}
 page.call("deny","","")?;
 let after=page.wait(|d|events(d).iter().any(|e|e["type"]=="read"))?;
 println!("PROOF unattended_read label={label} before={before} after={after}");
 if events(&after).iter().any(|e|e["type"]=="read"&&e["result"]!="DENIED"){return Err(format!("unattended read allowed: {label}"));}
 Ok(())
}
pub(super) fn run(a:&Page,b:&Page,shuttle:&Page)->Result<(),String>{
 assert_ne!(std::env::var("PROOF_INJECT_ASSERT").as_deref(),Ok("1"),"deliberate cleanup-proof assertion");
 // These checks precede any injected gesture. Native paste consent is NOT
 // interpreted as a prohibition on reads in that explicit paste event.
 deny(a,"A-focused-nonactivated",true)?;
 a.focus(true);deny(b,"B-independent-background",false)?;
 let mut semantic_count=0;
 let mut semantic_failures=0;
 for repeat in 0..10{
  for operation in ["copy","cut"]{
   for mode in ["default","custom","cancel","noselect"]{
    let sentinel=unique("SENTINEL");seed(shuttle,&sentinel)?;
    prepare(shuttle)?;a.set(UNICODE,if mode=="noselect"{"default"}else{mode})?;
    if mode=="noselect"{a.call("noselect","","")?;}
    a.native(operation)?; // renderer Frame API returns, then native IPC ACK
    let actual=extract(shuttle)?; // exactly ONE extraction, no expected-value poll
    let source=a.snapshot()?; // proof only, deliberately AFTER extraction
    let expected=match mode{"custom"=>"CUSTOM 🦀\n exact","cancel"|"noselect"=>sentinel.as_str(),_=>UNICODE};
    if actual["value"]!=expected{return Err(format!("single extraction mismatch {repeat}/{operation}/{mode}: {actual}; source={source}"));}
    let export=if actual["value"]==sentinel{None}else{actual["value"].as_str()};
    if matches!(mode,"cancel"|"noselect")&&export.is_some(){return Err("no-write operation exported text".into());}
    if mode!="noselect"&&!has_event(&source,operation){return Err(format!("missing trusted {operation}: {source}"));}
    if source["value"]!=if operation=="cut"&&mode=="default"{""}else{UNICODE}{return Err(format!("wrong cut semantics {source}"));}
    if mode=="cancel"&&!events(&source).iter().any(|e|e["type"]==operation&&e["prevented"]==true){return Err("cancel event not prevented".into());}
    println!("PROOF semantic repeat={repeat} operation={operation} mode={mode} sentinel={sentinel:?} export={export:?} source={source} shuttle={actual}");
    semantic_count+=1;
   }
  }
  for mode in ["default","cancel"]{
   seed(shuttle,UNICODE)?;shuttle.focus(false);a.set("REPLACE",mode)?;a.native("paste")?;
   let source=a.wait(|d|has_event(d,"paste")&&(mode=="cancel"||has_event(d,"input")))?;
   let paste=events(&source).into_iter().find(|e|e["type"]=="paste").unwrap();
   if paste["data"]!=UNICODE || source["value"]!=if mode=="cancel"{"REPLACE"}else{UNICODE}{
    semantic_failures+=1;
    println!("PROOF runtime_missing_contract renderer_paste_exact_data=false repeat={repeat} mode={mode} source={source}");
   }
   if mode=="cancel"&&paste["prevented"]!=true{return Err("paste cancellation missing".into());}
   println!("PROOF semantic repeat={repeat} operation=paste mode={mode} source={source}");
   semantic_count+=1;
  }
 }
 println!("PROOF semantic_complete repeats=10 cases={semantic_count} failed={semantic_failures} completion=renderer_return_ack_then_single_shuttle_paste empirical_only=true");
 // Deterministic interleaving at the proposed bridge's extraction boundary.
 // B is an unrelated owned context receiving a separately authorized native
 // Copy action. No website grant, cross-page binding, or global override.
 let mut attribution_failures=0;
 for operation in ["copy","cut"]{
  for mode in ["cancel","noselect"]{
   let sentinel=unique("COMPETING_SENTINEL");let writer=unique("OWNED_SOURCE_B 🦀\n exact\t ");
   seed(shuttle,&sentinel)?;prepare(shuttle)?;
   a.set(UNICODE,if mode=="noselect"{"default"}else{mode})?;
   if mode=="noselect"{a.call("noselect","","")?;}
   a.native(operation)?;
   let no_write=a.snapshot()?;
   // Establish the no-write observation separately for this controlled test.
   let before_b=extract(shuttle)?;
   if before_b["value"]!=sentinel || no_write["value"]!=UNICODE{return Err(format!("A no-write precondition failed {no_write} {before_b}"));}
   prepare(shuttle)?;
   b.set(&writer,"default")?;b.native("copy")?;
   let extracted=extract(shuttle)?;
   let writer_snapshot=b.snapshot()?;
   if extracted["value"]!=writer || !has_event(&writer_snapshot,"copy"){return Err(format!("competing writer not observed {extracted} {writer_snapshot}"));}
   // Model ONLY the rejected sentinel inequality decision, never export to OS.
   let old_algorithm_would_export=extracted["value"]!=sentinel;
   if !old_algorithm_would_export{return Err("counterexample not established".into());}
   attribution_failures+=1;
   println!("PROOF runtime_missing_contract operation={operation} mode={mode} owner_A_no_write=true writer_attribution=owned_B_native_copy old_algorithm_would_export_wrong_workspace={old_algorithm_would_export} sentinel={sentinel:?} A={no_write} pre_B={before_b} B={writer_snapshot} extracted={extracted}");
  }
 }
 // Normal gesture-activated website write is measured, not mislabeled as a
 // strict-all-API isolation violation. Input goes ONLY to this owned CEF page.
 b.set("gesture fixture","default")?;
 let mouse=MouseEvent{x:20,y:20,modifiers:0};
 for up in [0,1]{b.browser.host().unwrap().send_mouse_click_event(Some(&mouse),MouseButtonType::LEFT,up,1);}
 let activated=b.wait(|d|d["activation"]==true)?;
 let gesture_text=unique("OWNED_GESTURE_WRITE");b.call("write",&gesture_text,"")?;
 let write=b.wait(|d|events(d).iter().any(|e|e["type"]=="write"))?;
 println!("PROOF normal_useractivated_website_write activation={activated} result={write} allowed_is_not_blocker=true");
 if events(&write).iter().any(|e|e["type"]=="write"&&e["result"]=="ALLOWED"){
  let actual=clipboard_read(shuttle)?;
  if actual["value"]!=gesture_text{return Err(format!("activated write extraction mismatch {actual}"));}
  println!("PROOF normal_useractivated_website_write_exact_readback={actual}");
 }
 println!("PROOF runtime_missing_contract counterexamples={attribution_failures} semantic_failures={semantic_failures} production_acceptance=false no_OS_clipboard_claim=true");
 Err(format!("runtime_missing_contract: {attribution_failures} wrong-owner counterexamples; {semantic_failures} semantic failures"))
}
