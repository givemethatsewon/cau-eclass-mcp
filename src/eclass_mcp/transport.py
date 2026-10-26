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

from .browser_scripts import READ_SCRIPT, SUBMIT_SCRIPT, VIDEO_SCRIPT

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
            """await (async()=>{let bridgeStage="initialization";try {
const found=(await listBrowserTabs()).find(t=>t.url.startsWith("https://eclass3.cau.ac.kr/"));
if(!found){console.log('ECLASS_RESULT:'+JSON.stringify({bridge_error:'Open a logged-in e-Class tab in Aside first'}));return;}
const p=await attachBrowserTab(found.targetId);
"""
            + body
            + """
} catch(e) {console.log('ECLASS_RESULT:'+JSON.stringify({bridge_error:'Browser operation failed at '+bridgeStage+'; refresh the e-Class session'}));}})();"""
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
const value=await f.evaluate("""
            + READ_SCRIPT
            + ","
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

    def collect_file(self, value, destination: Path):
        sources = [Path(part).resolve() for part in value.get("parts", [value.get("path")])]
        root = (Path.home() / ".aside").resolve()
        if not sources or any(not src.is_relative_to(root) or not src.is_file() for src in sources):
            raise EClassError("Downloads require a local Aside session")
        try:
            with destination.open("xb") as output:
                os.chmod(destination, 0o600)
                for src in sources:
                    with src.open("rb") as source:
                        shutil.copyfileobj(source, output)
        finally:
            for src in sources:
                src.unlink(missing_ok=True)
        return value

    async def video_original(
        self, course_id: int, item_id: int, destination: Path | None = None, allow_playback_copy: bool = False
    ):
        q = {
            "course": course_id,
            "item": item_id,
            "download": destination is not None,
            "allow_playback_copy": allow_playback_copy,
            "name": uuid4().hex + ".mp4",
            "limit": 1024 * 1024 * 1024,
        }
        code = self.wrap(
            """
const q="""
            + json.dumps(q)
            + r""";
bridgeStage="video metadata";
const media=await p.evaluate("""
            + VIDEO_SCRIPT
            + r""",q);
const {url,view_url,...info}=media;
if(!q.download||!info.available){console.log('ECLASS_RESULT:'+JSON.stringify(info));return;}
if(info.expected_bytes>q.limit)throw Error('size limit');
bridgeStage="original response";
let response=await fetch(url);let source='commons_original';
if(!response.ok){
await response.body.cancel();
if(!q.allow_playback_copy){console.log('ECLASS_RESULT:'+JSON.stringify({bridge_error:'Original download endpoint failed; optionally allow a playback MP4 copy'}));return;}
const trusted=(value)=>{const u=new URL(value);if(u.protocol!=='https:'||!['ocs.cau.ac.kr','cau-cms-object.cdn.gov-ntruss.com'].includes(u.hostname)||u.username||u.password)throw Error('host');return u.href;};
bridgeStage="playback viewer metadata";
const htmlResponse=await fetch(trusted(view_url));if(!htmlResponse.ok)throw Error('viewer');
const html=await htmlResponse.text();const match=html.match(/var contentUri\s*=\s*'([^']+)'/);
if(!match)throw Error('content metadata');
bridgeStage="playback XML metadata";
const xmlResponse=await fetch(trusted(new URL(match[1],view_url).href));if(!xmlResponse.ok)throw Error('content metadata');
const xml=await xmlResponse.text();const template=xml.match(/<media_uri method="progressive" target="all">([^<]+)/);
const files=[...xml.matchAll(/<main_media media_id="[^"]+">([^<]+)/g)];
if(!template||files.length!==1||!/^[-a-zA-Z0-9_.]+\.mp4$/.test(files[0][1]))throw Error('unsupported media');
bridgeStage="MP4 response";
response=await fetch(trusted(template[1].replace('[MEDIA_FILE]',files[0][1])));
source='commons_playback_mp4';info.expected_bytes=null;
}
const ct=response.headers.get('content-type')||'';
if(!response.ok||ct.includes('html')||ct.includes('json'))throw Error('not media');
if(Number(response.headers.get('content-length')||0)>q.limit)throw Error('size limit');
const dir=path.join(pwd,'artifacts');await fs.mkdir(dir,{recursive:true});const out=path.join(dir,q.name);
bridgeStage="local file creation";
const parts=[];let chunks=[];let buffered=0;let size=0;let success=false;
const reader=response.body.getReader();
async function flush(){if(!buffered)return;const part=out+'.'+parts.length;await fs.writeFile(part,Buffer.concat(chunks));parts.push(part);chunks=[];buffered=0;}
try {for(;;){const part=await reader.read();if(part.done)break;size+=part.value.length;
if(size>q.limit)throw Error('size limit');chunks.push(Buffer.from(part.value));buffered+=part.value.length;if(buffered>=8*1024*1024)await flush();}
await flush();if(!size||(info.expected_bytes&&size!==info.expected_bytes))throw Error('size mismatch');success=true;
} finally {if(!success){await reader.cancel();for(const part of parts)await fs.unlink(part);}}
console.log('ECLASS_RESULT:'+JSON.stringify({...info,source,parts,bytes:size,content_type:ct}));
"""
        )
        value = await self.run(code)
        if destination is not None and value.get("available"):
            return self.collect_file(value, destination)
        return value

    async def syllabus_pdf(self, course_id: int, destination: Path):
        code = self.wrap(
            """
const q="""
            + json.dumps({"course": course_id, "name": uuid4().hex + ".pdf"})
            + """;
const tab=await openTab('https://eclass3.cau.ac.kr/courses/'+q.course+'/assignments/syllabus');
try {
await snapshot(tab,{interactive:true});
const frame=tab.frames().find(f=>f.url().startsWith('https://rpt80.cau.ac.kr/oz80/ozViewer.jsp'));
if(!frame)throw Error('OZ viewer unavailable');
await frame.locator('input.btnSAVEAS[title="Save"]').click({timeout:30000});
await frame.locator('[role="dialog"] select.oz_ui_layout').selectOption({label:'Adobe PDF File(*.pdf)'});
const pending=tab.waitForEvent('download',{timeout:60000});
await frame.getByRole('button',{name:'OK',exact:true}).click();
const download=await pending;
const dir=path.join(pwd,'artifacts');await fs.mkdir(dir,{recursive:true});
const out=path.join(dir,q.name);await download.saveAs(out);
if(await download.failure())throw Error('download failed');
const stat=await fs.stat(out);
console.log('ECLASS_RESULT:'+JSON.stringify({path:out,bytes:stat.size,content_type:'application/pdf'}));
} finally {await closeTab(tab);}
"""
        )
        return self.collect_file(await self.run(code), destination)

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
const result=await p.evaluate("""
            + SUBMIT_SCRIPT
            + """,q);console.log('ECLASS_RESULT:'+JSON.stringify(result));"""
        )
        return await self.run(self.wrap(body))
