READ_SCRIPT = r"""async (q)=>{
const headers={Accept:q.html?'text/html':'application/json+canvas-string-ids'};
if(q.learningx){const m=document.cookie.match('(^|;) ?xn_api_token=([^;]*)(;|$)');
 if(!m)return {status:401,body:null,content_type:'',next:null};headers.Authorization='Bearer '+m[2];}
const res=await fetch(q.path,{method:'GET',headers,credentials:'same-origin',redirect:'error'});
const text=await res.text();let body;
const secret=/token|secret|signature|verifier|authorization|password|nonce|secure_params|credential|csrf|oauth|access_key/i;
const publicKeys=new Set(['wrap','page','per_page','include[]','start_date','end_date','ozr_path','ozr_nm','param_odi','param_form']);
const safeUrl=(s)=>{try{const u=new URL(s);u.username='';u.password='';u.hash='';
 for(const key of [...u.searchParams.keys()])if(!publicKeys.has(key))u.searchParams.set(key,'[REDACTED]');return u.href;}catch{return '[invalid URL]';}};
const scrub=(x)=>{if(Array.isArray(x))return x.map(scrub);
 if(x&&typeof x==='object')return Object.fromEntries(Object.entries(x).filter(([k])=>!secret.test(k)).map(([k,v])=>[k,scrub(v)]));
 if(typeof x==='string')return x.replace(/https?:\/\/[^\s<>"']+/g,safeUrl);return x;};
try{if(q.html){const doc=new DOMParser().parseFromString(text,'text/html');
 body=[...doc.querySelectorAll('iframe[src]')].map(el=>{const out=doc.createElement('iframe');out.setAttribute('src',safeUrl(el.getAttribute('src')));return out.outerHTML;}).join('');
 }else body=scrub(JSON.parse(text.replace(/^while\(1\);\s*/,'')));}
catch{body=null;}
return {status:res.status,body,content_type:res.headers.get('content-type'),link:res.headers.get('link')};
}"""

VIDEO_SCRIPT = r"""async(q)=>{
const m=document.cookie.match('(^|;) ?xn_api_token=([^;]*)(;|$)');if(!m)throw Error('login');
const headers={Authorization:'Bearer '+m[2]};
const r=await fetch('/learningx/api/v1/courses/'+q.course+'/modules?include_detail=true',{headers,credentials:'same-origin',redirect:'error'});
if(!r.ok)throw Error('modules');const modules=await r.json();
function find(x){if(!x||typeof x!=='object')return null;
if(String(x.item_id)===String(q.item)&&x.item_content_data)return x;
for(const v of Object.values(x)){const found=find(v);if(found)return found;}return null;}
const item=find(modules);if(!item)return {available:false,reason:'Video item not found in course'};
const data=item.item_content_data;
if(item.opened===false||!data.content_id||data.content_id==='not_open')return {available:false,reason:'Video is not open to this user'};
if(data.content_type!=='mp4')return {available:false,reason:'Only original MP4 downloads are supported'};
const response=await fetch('/learningx/api/v1/commons/contents?content_id='+encodeURIComponent(data.content_id),{headers,credentials:'same-origin',redirect:'error'});
if(!response.ok)throw Error('metadata');const meta=(await response.json()).result;
if(!meta||meta.is_original_file_deleted||!meta.download_url)return {available:false,reason:'Original file is unavailable'};
const u=new URL(meta.download_url);
if(u.protocol!=='https:'||u.hostname!=='ocs.cau.ac.kr'||u.username||u.password)throw Error('download host');
return {available:true,title:item.title,expected_bytes:Number(meta.original_file_length)||null,url:u.href,view_url:meta.view_url};
}"""

SUBMIT_SCRIPT = r"""async(q)=>{
const token=document.cookie.match('(^|;) ?_csrf_token=([^;]*)(;|$)');
if(!token)throw Error('csrf unavailable');
const headers={'X-CSRF-Token':decodeURIComponent(token[2]),'Content-Type':'application/json','Accept':'application/json'};
const parse=async(r)=>{if(!r.ok)throw Error('request failed');return JSON.parse((await r.text()).replace(/^while\(1\);\s*/,''));};
const base='/api/v1/courses/'+q.course+'/assignments/'+q.assignment+'/submissions';
const ids=[];
for(const file of q.files){
 const bytes=Uint8Array.from(atob(file.data),c=>c.charCodeAt(0));
 const ticket=await parse(await fetch(base+'/self/files',{method:'POST',credentials:'same-origin',redirect:'error',headers,body:JSON.stringify({name:file.name,size:bytes.length,content_type:file.type})}));
 const target=new URL(ticket.upload_url);if(target.protocol!=='https:'||target.username||target.password)throw Error('unsafe upload target');
 const form=new FormData();for(const [key,value] of Object.entries(ticket.upload_params||{}))form.append(key,String(value));
 form.append('file',new Blob([bytes],{type:file.type}),file.name);
 // Never forward Canvas credentials or Authorization to the upload host.
 const uploaded=await parse(await fetch(target.href,{method:'POST',body:form,credentials:'omit',redirect:'error'}));
 if(!uploaded.id)throw Error('upload finalization requires unsupported redirect');ids.push(uploaded.id);
}
const submission=await parse(await fetch(base,{method:'POST',credentials:'same-origin',redirect:'error',headers,body:JSON.stringify({submission:{submission_type:'online_upload',file_ids:ids}})}));
return {file_ids:ids,submission_id:submission.id};
}"""
