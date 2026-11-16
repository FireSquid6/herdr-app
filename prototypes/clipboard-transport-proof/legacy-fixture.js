(function(op,text,mode){
 // Fixed OWNED fixture value only. Never log raw clipboard contents.
 const seed='OWNED_LEGACY_READ_SECRET 🦀\n exact\t ';
 let e=document.getElementById('editor');
 let log=document.getElementById('events');
 const append=row=>{log.textContent+=JSON.stringify(row)+'\n';};
 if(op==='init'){
  if(!e){e=document.createElement('textarea');e.id='editor';document.body.append(e);}
  if(!log){log=document.createElement('pre');log.id='events';document.body.append(log);}
  if(!e.dataset.installed){e.dataset.installed='1';
   for(const type of ['copy','cut','paste','beforeinput','input']) e.addEventListener(type,event=>{
    const data=event.clipboardData?.getData('text/plain');
    if(type==='paste'&&e.dataset.mode==='cancel')event.preventDefault();
    append({type,trusted:event.isTrusted,inputType:event.inputType??null,
     dataPresent:data!==undefined,dataEmpty:data===undefined?null:data.length===0,
     dataLength:data===undefined?null:data.length,dataExactSeed:data===seed,
     prevented:event.defaultPrevented});
   });
  }
 }
 if(op==='set'){
  e.value=text;e.dataset.mode=mode;e.focus();e.select();log.textContent='';
  delete e.dataset.legacyResult;delete e.dataset.legacyThrew;
 }
 if(op==='select'){e.focus();e.select();}
 if(op==='legacy'){
  try{e.dataset.legacyResult=String(document.execCommand('paste'));e.dataset.legacyThrew='false';}
  catch(_){e.dataset.legacyResult='null';e.dataset.legacyThrew='true';}
 }
 if(op==='reads'){
  for(const action of ['readText','read']){
   try{
    const promise=navigator.clipboard[action]();
    Promise.resolve(promise).then(value=>{
     // API success remains ALLOWED even if later data inspection fails.
     append({type:'api_read',action,result:'ALLOWED'});
     const inspect=async()=>{
      let texts=[];
      if(action==='readText')texts=[value];
      else for(const item of value){if(item.types.includes('text/plain'))texts.push(await(await item.getType('text/plain')).text());}
      append({type:'api_read_data',action,sawSeed:texts.some(t=>t.includes(seed)),lengths:texts.map(t=>t.length)});
     };
     inspect().catch(()=>append({type:'api_read_data_error',action}));
    },()=>append({type:'api_read',action,result:'DENIED',sawSeed:false}));
   }catch(_){append({type:'api_read',action,result:'DENIED',sawSeed:false});}
  }
 }
 return JSON.stringify({events:log.textContent,focus:document.hasFocus(),activation:navigator.userActivation.isActive,
  editableFocused:document.activeElement===e,selection:[e.selectionStart,e.selectionEnd],
  valueEmpty:e.value.length===0,valueLength:e.value.length,valueUnchanged:e.value==='UNCHANGED',
  valueExactSeed:e.value===seed,domSawSeed:e.value.includes(seed)||document.body.textContent.includes(seed),
  legacyResult:e.dataset.legacyResult===undefined?null:e.dataset.legacyResult==='true',
  legacyThrew:e.dataset.legacyThrew==='true'});
})
