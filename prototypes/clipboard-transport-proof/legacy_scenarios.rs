use super::*;
const SEED:&str="OWNED_LEGACY_READ_SECRET 🦀\n exact\t ";
fn establish_seed(shuttle:&Page)->Result<(),String>{
 shuttle.set(SEED,"default")?;
 shuttle.browser.main_frame().ok_or("missing shuttle frame")?.copy();
 shuttle.wait(|d|has_event(d,"copy"))?;
 let start=Instant::now();
 loop{
  shuttle.set("","default")?;
  shuttle.browser.main_frame().unwrap().paste();
  let data=shuttle.wait(|d|has_event(d,"paste"))?;
  if data["valueExactSeed"]==true{
   println!("LEGACY_SEED exact_readback=true");shuttle.focus(false);return Ok(());
  }
  if start.elapsed()>Duration::from_secs(2){return Err("owned seed readback failed (contents not logged)".into());}
 }
}
fn record(label:&str,operation:&str,before:&serde_json::Value,after:&serde_json::Value,pass:bool)->bool{
 println!("LEGACY_CASE {}",serde_json::json!({"label":label,"operation":operation,"before":before,"after":after,"pass":pass}));pass
}
pub(super) fn run(a:&Page,b:&Page,shuttle:&Page)->Result<(),String>{
 establish_seed(shuttle)?;
 let mut passed=0;let mut attempted=0;
 for (page,label,focus,activation) in [(a,"A-focused-nonactivated",true,false),(a,"A-focused-click-activated",true,true),(b,"B-independent-background",false,false)]{
  shuttle.focus(false);
  if label.starts_with('B'){a.focus(true);}
  page.set("UNCHANGED","default")?;page.focus(focus);
  if activation{
   let mouse=MouseEvent{x:20,y:20,modifiers:0};
   for up in [0,1]{page.browser.host().unwrap().send_mouse_click_event(Some(&mouse),MouseButtonType::LEFT,up,1);}
   page.wait(|d|d["activation"]==true)?;
   // The actual click places a caret; reselect without granting activation.
   page.call("select","","")?;
  }
  let before=page.snapshot()?;
  let precondition=before["focus"]==focus&&before["activation"]==activation&&before["valueUnchanged"]==true&&before["selection"]==serde_json::json!([0,9]);
  if !precondition{return Err(format!("wrong focus/activation/selection precondition: {label} {before}"));}
  let legacy=page.call("legacy","","")?;
  let no_observation=legacy["domSawSeed"]==false&&events(&legacy).iter().all(|e|e["dataExactSeed"]!=true);
  let pass=legacy["legacyResult"]==false&&legacy["legacyThrew"]==false&&legacy["valueUnchanged"]==true&&no_observation&&events(&legacy).is_empty();
  attempted+=1;if record(label,"execCommand_paste",&before,&legacy,pass){passed+=1;}
  page.call("reads","","")?;
  let reads=page.wait(|d|events(d).iter().filter(|e|e["type"]=="api_read").count()==2)?;
  for action in ["readText","read"]{
   let rows=events(&reads);
   let row=rows.iter().find(|e|e["type"]=="api_read"&&e["action"]==action);
   let pass=row.is_some_and(|e|e["result"]=="DENIED")&&reads["valueUnchanged"]==true&&reads["domSawSeed"]==false&&rows.iter().all(|e|e["dataExactSeed"]!=true&&e["sawSeed"]!=true);
   attempted+=1;if record(label,action,&before,&reads,pass){passed+=1;}
  }
 }
 // Only NOW does A receive explicit native Paste consent, from the browser
 // process API. Do not substitute the previous renderer-side Frame.Paste.
 for mode in ["default","cancel"]{
  establish_seed(shuttle)?;a.set("UNCHANGED",mode)?;
  let before=a.snapshot()?;
  if before["focus"]!=true||before["editableFocused"]!=true{return Err("native paste target not focused".into());}
  a.browser.main_frame().ok_or("missing source frame")?.paste();
  let after=a.wait(|d|has_event(d,"paste")&&(mode=="cancel"||has_event(d,"input")))?;
  let rows=events(&after);let paste=rows.iter().find(|e|e["type"]=="paste").unwrap();
  let pass=paste["trusted"]==true&&paste["dataExactSeed"]==true&&paste["dataEmpty"]==false&&paste["prevented"]==(mode=="cancel")&&if mode=="cancel"{after["valueUnchanged"]==true}else{after["valueExactSeed"]==true&&has_event(&after,"beforeinput")&&has_event(&after,"input")};
  attempted+=1;if record(mode,"browser_native_Frame_Paste",&before,&after,pass){passed+=1;}
 }
 println!("LEGACY_TOTAL {}",serde_json::json!({"attempted":attempted,"passed":passed,"failed":attempted-passed,"production_accepted":false,"OS_clipboard_claim":false,"renderer_paste_empty_cause":"UNDETERMINED; not tested or corrected here"}));
 if passed!=attempted{return Err(format!("legacy read gate/native semantics: {} of {attempted} cases failed",attempted-passed));}
 Ok(())
}
