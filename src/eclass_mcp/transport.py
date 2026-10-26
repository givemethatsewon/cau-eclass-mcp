"""Private, persistent Aside MCP bridge. Credentials never leave the browser."""

from __future__ import annotations

import asyncio
import json
import os
import re
import shutil
from pathlib import Path
from urllib.parse import urlsplit
from uuid import uuid4

from fastmcp import Client
from fastmcp.client.transports import StdioTransport

ORIGIN = "https://eclass3.cau.ac.kr"


class EClassError(Exception):
    pass


def local_path(value: str) -> str:
    parsed = urlsplit(value)
    if parsed.scheme or parsed.netloc or not value.startswith("/") or value.startswith("//"):
        raise EClassError("Only same-origin relative paths are allowed")
    if re.search(r"(?i)%2e|%2f|%5c", parsed.path) or "\\" in value or ".." in parsed.path.split("/"):
        raise EClassError("Invalid path")
    return value


class AsideTransport:
    def __init__(self):
        self.client = Client(
            StdioTransport(command=os.getenv("ECLASS_ASIDE_BIN", "aside"), args=["mcp"]), timeout=120
        )
        self.lock = asyncio.Lock()
        self.profile = os.getenv("ECLASS_ASIDE_PROFILE")

    async def __aenter__(self):
        await self.client.__aenter__()
        return self

    async def __aexit__(self, *args):
        return await self.client.__aexit__(*args)

    async def run(self, code: str):
        async with self.lock:
            args = {"code": code, "title": "e-Class MCP"}
            if self.profile:
                args["profile_id"] = self.profile
            try:
                result = await self.client.call_tool("repl", args)
            except Exception:
                raise EClassError("Aside request failed; check Aside and the logged-in e-Class tab") from None
            for block in result.content:
                txt = getattr(block, "text", "")
                marker = "ECLASS_RESULT:"
                if marker in txt:
                    try:
                        value, _ = json.JSONDecoder().raw_decode(txt.split(marker, 1)[1])
                        if value.get("bridge_error"):
                            raise EClassError(value["bridge_error"])
                        return value
                    except json.JSONDecodeError:
                        pass
            # Do not echo Aside exceptions: they can contain URLs or page contents.
            raise EClassError("Aside returned no structured result")

    @staticmethod
    def wrap(body: str) -> str:
        return (
            """await (async()=>{try {
const found=(await listBrowserTabs()).find(t=>t.url.startsWith("https://eclass3.cau.ac.kr/"));
if(!found){console.log('ECLASS_RESULT:'+JSON.stringify({bridge_error:'Open a logged-in e-Class tab in Aside first'}));return;}
const p=await attachBrowserTab(found.targetId);
"""
            + body
            + """
} catch(e) {console.log('ECLASS_RESULT:'+JSON.stringify({bridge_error:'Browser operation failed; refresh the e-Class session'}));}})();"""
        )

    async def get(self, path: str, learningx: bool = False, html: bool = False):
        payload = json.dumps({"path": local_path(path), "learningx": learningx, "html": html})
        code = self.wrap(
            """
const frames=p.frames();
const f="""
            + (
                'frames.find(f=>f.url().startsWith("https://eclass3.cau.ac.kr/learningx/")) || p'
                if learningx
                else "p"
            )
            + """;
const value=await f.evaluate(async (q)=>{
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
 if(typeof x==='string')return x.replace(/https?:\\/\\/[^\\s<>"']+/g,safeUrl);return x;};
try{if(q.html){const doc=new DOMParser().parseFromString(text,'text/html');
 body=[...doc.querySelectorAll('iframe[src]')].map(el=>{const out=doc.createElement('iframe');out.setAttribute('src',safeUrl(el.getAttribute('src')));return out.outerHTML;}).join('');
 }else body=scrub(JSON.parse(text.replace(/^while\\(1\\);\\s*/,'')));}
catch{body=null;}
return {status:res.status,body,content_type:res.headers.get('content-type'),link:res.headers.get('link')};
},"""
            + payload
            + """);console.log('ECLASS_RESULT:'+JSON.stringify(value));"""
        )
        value = await self.run(code)
        if value["status"] >= 400:
            raise EClassError(
                f"e-Class HTTP {value['status']} (session, permission, or unsupported endpoint)"
            )
        if value["body"] is None:
            raise EClassError("Expected JSON was not returned; login may have expired")
        return value

    async def download(self, path: str, destination: Path, max_bytes: int):
        payload = json.dumps(
            {"url": ORIGIN + local_path(path), "name": uuid4().hex + ".download", "limit": max_bytes}
        )
        code = self.wrap(
            """const q="""
            + payload
            + """;
const r=await fetch(q.url);
const ct=r.headers.get('content-type')||'';
if(!r.ok||ct.includes('text/html')||ct.includes('application/json')){
console.log('ECLASS_RESULT:'+JSON.stringify({bridge_error:'Download did not return a file'}));return;}
if(Number(r.headers.get('content-length')||0)>q.limit){console.log('ECLASS_RESULT:'+JSON.stringify({bridge_error:'File exceeds download limit'}));return;}
const reader=r.body.getReader();const chunks=[];let size=0;
for(;;){const part=await reader.read();if(part.done)break;size+=part.value.length;
if(size>q.limit){await reader.cancel();throw Error('limit');}chunks.push(Buffer.from(part.value));}
const dir=path.join(pwd,'artifacts');await fs.mkdir(dir,{recursive:true});
const out=path.join(dir,q.name);await fs.writeFile(out,Buffer.concat(chunks));
console.log('ECLASS_RESULT:'+JSON.stringify({path:out,bytes:size,content_type:ct}));"""
        )
        value = await self.run(code)
        src = Path(value["path"]).resolve()
        aside_root = (Path.home() / ".aside").resolve()
        if not src.is_relative_to(aside_root) or not src.is_file():
            raise EClassError("Downloads require a local Aside session")
        # Exclusive create prevents overwrite races and symlink replacement.
        with destination.open("xb") as output, src.open("rb") as input_file:
            os.chmod(destination, 0o600)
            shutil.copyfileobj(input_file, output)
        src.unlink()
        return value

    async def submit_files(self, plan):
        """Write path. Called only after the service's opt-in and plan checks."""
        import base64
        import mimetypes

        payload = {
            "course": plan["course_id"],
            "assignment": plan["assignment_id"],
            "files": [
                {
                    "name": f["name"],
                    "type": mimetypes.guess_type(f["name"])[0] or "application/octet-stream",
                    "data": base64.b64encode(Path(f["path"]).read_bytes()).decode(),
                }
                for f in plan["files"]
            ],
        }
        body = (
            """const q="""
            + json.dumps(payload)
            + """;
const result=await p.evaluate(async(q)=>{
const token=document.cookie.match('(^|;) ?_csrf_token=([^;]*)(;|$)');
if(!token)throw Error('csrf unavailable');
const headers={'X-CSRF-Token':decodeURIComponent(token[2]),'Content-Type':'application/json','Accept':'application/json'};
const parse=async(r)=>{if(!r.ok)throw Error('request failed');return JSON.parse((await r.text()).replace(/^while\\(1\\);\\s*/,''));};
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
},q);console.log('ECLASS_RESULT:'+JSON.stringify(result));"""
        )
        return await self.run(self.wrap(body))
