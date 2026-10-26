from __future__ import annotations

import asyncio
import base64
import hashlib
import mimetypes
import os
import re
from datetime import datetime, timedelta, timezone
from pathlib import Path
from urllib.parse import parse_qsl, urlencode, urlsplit, urlunsplit
from uuid import uuid4
from zoneinfo import ZoneInfo

from bs4 import BeautifulSoup

from .transport import ORIGIN, EClassError, local_path

SECRET = re.compile(
    r"token|secret|signature|verifier|authorization|password|nonce|secure_params|credential|csrf|oauth|access_key",
    re.I,
)


def safe_url(url: str) -> str:
    p = urlsplit(url)
    if p.scheme not in ("http", "https", ""):
        return "[unsupported URL]"
    public_parameters = {
        "wrap",
        "page",
        "per_page",
        "include[]",
        "start_date",
        "end_date",
        "ozr_path",
        "ozr_nm",
        "param_odi",
        "param_form",
    }
    query = [
        (k, v if k in public_parameters else "[REDACTED]")
        for k, v in parse_qsl(p.query, keep_blank_values=True)
    ]
    return urlunsplit((p.scheme, p.netloc.split("@")[-1], p.path, urlencode(query), ""))


def clean(value):
    if isinstance(value, dict):
        return {k: clean(v) for k, v in value.items() if not SECRET.search(k)}
    if isinstance(value, list):
        return [clean(v) for v in value]
    if isinstance(value, str):
        return re.sub(r'https?://[^\s<>"\']+', lambda m: safe_url(m[0]), value)
    return value


def text(html):
    soup = BeautifulSoup(html or "", "html.parser")
    for element in soup(["script", "style", "input"]):
        element.decompose()
    return soup.get_text(" ", strip=True)


def iso_now():
    return datetime.now(timezone.utc).isoformat()


def date_fields(item):
    result = {}
    for key in ("unlock_at", "late_at", "due_at", "lock_at"):
        value = item.get(key)
        result[key] = value
        if value:
            try:
                result[key + "_seoul"] = (
                    datetime.fromisoformat(value.replace("Z", "+00:00"))
                    .astimezone(ZoneInfo("Asia/Seoul"))
                    .isoformat()
                )
            except ValueError:
                result[key + "_seoul"] = None
    return result


def select(obj, *keys):
    return {k: obj[k] for k in keys if k in obj}


class EClassService:
    def __init__(self, transport, output_dir: Path | None = None, enable_submission: bool = False):
        self.transport = transport
        default_output = Path.home() / "Downloads" / "cau-eclass"
        self.output_dir = (
            output_dir or Path(os.getenv("ECLASS_DOWNLOAD_DIR", str(default_output)))
        ).resolve()
        self.enable_submission = enable_submission
        self.plans = {}
        self.submission_lock = asyncio.Lock()

    async def get(self, path, lx=False, html=False):
        return (await self.transport.get(path, learningx=lx, html=html))["body"]

    async def paged(self, path, lx=False, max_pages=20):
        items, seen = [], set()
        for _ in range(max_pages):
            if path in seen:
                raise EClassError("Pagination loop detected")
            seen.add(path)
            response = await self.transport.get(path, learningx=lx)
            body = response["body"]
            if not isinstance(body, list):
                raise EClassError("Unexpected list response; completeness cannot be established")
            items.extend(body)
            link = response.get("link") or ""
            match = re.search(r'<([^>]+)>;\s*rel="next"', link)
            if not match:
                return {"items": items, "complete": True, "next_cursor": None}
            u = urlsplit(match[1])
            if u.scheme and f"{u.scheme}://{u.netloc}" != ORIGIN:
                raise EClassError("Cross-origin pagination rejected")
            path = local_path(u.path + ("?" + u.query if u.query else ""))
        return {"items": items, "complete": False, "next_cursor": path}

    async def session_status(self):
        user = await self.get("/api/v1/users/self/profile")
        if not isinstance(user, dict) or not user.get("id"):
            raise EClassError("No authenticated user profile was returned")
        return {
            "authenticated": True,
            "user": select(user, "id", "name"),
            "submission_enabled": self.enable_submission,
        }

    async def courses(self):
        result = await self.paged("/api/v1/courses?include[]=term&per_page=100")
        result["items"] = [
            select(c, "id", "name", "course_code", "workflow_state", "term") for c in result["items"]
        ]
        return result

    async def modules(self, course_id):
        modules = await self.get(f"/learningx/api/v1/courses/{course_id}/modules?include_detail=true", True)
        if not isinstance(modules, list):
            raise EClassError("Unexpected LearningX module response")
        return {"items": modules, "complete": True, "next_cursor": None}

    async def announcements(self, course_id, start_date, end_date):
        return await self.paged(
            "/api/v1/announcements?"
            + urlencode(
                {
                    "context_codes[]": f"course_{course_id}",
                    "start_date": start_date,
                    "end_date": end_date,
                    "per_page": 100,
                }
            )
        )

    async def announcement(self, course_id, topic_id):
        return await self.get(f"/api/v1/courses/{course_id}/discussion_topics/{topic_id}")

    async def assignments(self, course_id):
        return await self.paged(f"/api/v1/courses/{course_id}/assignments?include[]=submission&per_page=100")

    async def assignment(self, course_id, assignment_id):
        return await self.get(f"/api/v1/courses/{course_id}/assignments/{assignment_id}?include[]=submission")

    async def submission(self, course_id, assignment_id):
        return await self.get(f"/api/v1/courses/{course_id}/assignments/{assignment_id}/submissions/self")

    async def deadlines(self, course_id):
        assignments = await self.assignments(course_id)
        items = [
            dict(
                kind="assignment",
                id=a["id"],
                title=a["name"],
                **date_fields(a),
                locked_for_user=a.get("locked_for_user"),
            )
            for a in assignments["items"]
        ]
        warnings = []
        try:
            modules = await self.modules(course_id)
            for module in modules["items"]:
                for entry in module.get("module_items", []):
                    data = entry.get("content_data") or {}
                    if entry.get("content_type") == "attendance_item" and any(
                        data.get(k) for k in ("due_at", "lock_at", "late_at")
                    ):
                        items.append(
                            dict(
                                kind="learning_item",
                                id=data.get("item_id"),
                                title=entry.get("title"),
                                use_attendance=data.get("use_attendance"),
                                **date_fields(data),
                            )
                        )
        except EClassError as error:
            warnings.append(str(error))
        return {
            "items": items,
            "complete": assignments["complete"] and not warnings,
            "warnings": warnings,
            "next_cursor": assignments["next_cursor"],
        }

    async def syllabus(self, course_id):
        course = await self.get(f"/api/v1/courses/{course_id}?include[]=syllabus_body")
        html = course.get("syllabus_body") or ""
        soup = BeautifulSoup(html, "html.parser")
        viewers = [safe_url(f["src"]) for f in soup.select("iframe[src]")]
        if not viewers:
            try:
                page = await self.get(f"/courses/{course_id}/assignments/syllabus", html=True)
                viewers = [
                    safe_url(f["src"])
                    for f in BeautifulSoup(page, "html.parser").select("iframe[src]")
                    if "rpt80.cau.ac.kr" in f["src"]
                ]
            except EClassError:
                pass
        return {
            "course_id": course_id,
            "text": text(html),
            "viewer_urls": viewers,
            "complete": not viewers and bool(text(html)),
            "warnings": ["External viewer document content is not extracted"] if viewers else [],
        }

    async def syllabus_document(self, course_id, extract_text=True):
        from pypdf import PdfReader

        self.output_dir.mkdir(parents=True, exist_ok=True)
        destination = self.output_dir / f"syllabus-{course_id}-{uuid4().hex[:8]}.pdf"
        await self.transport.syllabus_pdf(course_id, destination)
        try:
            if destination.stat().st_size > 64 * 1024 * 1024:
                raise ValueError("oversize PDF")
            with destination.open("rb") as stream:
                if stream.read(5) != b"%PDF-":
                    raise ValueError("invalid PDF")
            reader = PdfReader(destination)
            pages = [page.extract_text() or "" for page in reader.pages] if extract_text else []
            if not len(reader.pages):
                raise ValueError("empty PDF")
            result = {
                "course_id": course_id,
                "path": str(destination),
                "bytes": destination.stat().st_size,
                "sha256": hashlib.sha256(destination.read_bytes()).hexdigest(),
                "content_type": "application/pdf",
                "source": "oz_native_export",
                "page_count": len(reader.pages),
                "text": "\n\n".join(pages) if extract_text else None,
                "pages": [{"page": i + 1, "text": value} for i, value in enumerate(pages)],
                "text_complete": all(value.strip() for value in pages) if extract_text else None,
            }
            return result
        except Exception:
            destination.unlink(missing_ok=True)
            raise EClassError("Syllabus PDF validation or text extraction failed") from None

    async def boards(self, course_id):
        body = await self.get(f"/learningx/api/v1/learningx_board/courses/{course_id}/boards", True)
        return {
            "data": body,
            "complete": isinstance(body, list),
            "warnings": [] if isinstance(body, list) else ["Provider pagination schema requires inspection"],
        }

    async def board_posts(self, course_id, board_id, page):
        body = await self.get(
            f"/learningx/api/v1/learningx_board/courses/{course_id}/boards/{board_id}/posts?page={page}", True
        )
        pagination = body.get("pagination", {}) if isinstance(body, dict) else {}
        current, last = pagination.get("current_page"), pagination.get("last_page")
        known = isinstance(current, int) and isinstance(last, int)
        return {
            "data": body,
            "page": page,
            "complete": known and current == 1 and last == 1,
            "has_more": current < last if known else None,
            "next_page": current + 1 if known and current < last else None,
            "warnings": [] if known else ["Provider pagination could not be established"],
        }

    async def board_post(self, course_id, board_id, post_id):
        return await self.get(
            f"/learningx/api/v1/learningx_board/courses/{course_id}/boards/{board_id}/posts/{post_id}", True
        )

    async def groups(self):
        return await self.paged("/api/v1/users/self/groups?per_page=100")

    async def group_activity(self, group_id):
        # Verify membership, not merely a caller-supplied group ID.
        groups = await self.groups()
        if not any(str(g["id"]) == str(group_id) for g in groups["items"]):
            raise EClassError("Group is not in the fetched current-user groups")
        return {
            "discussions": await self.paged(f"/api/v1/groups/{group_id}/discussion_topics?per_page=100"),
            "files": await self.paged(f"/api/v1/groups/{group_id}/files?per_page=100"),
        }

    async def video_info(self, course_id):
        modules = await self.modules(course_id)
        videos = []
        for module in modules["items"]:
            for item in module.get("module_items", []):
                d = item.get("content_data") or {}
                media = d.get("item_content_data") or {}
                if not isinstance(media, dict):
                    continue
                if media.get("content_type") in ("mp4", "everlec", "movie360", "oncast", "zoom"):
                    videos.append(
                        {
                            "title": item.get("title"),
                            "item_id": d.get("item_id"),
                            "content_type": media.get("content_type"),
                            "duration": media.get("duration"),
                            "has_view_url": bool((media.get("view_url") or "").strip()),
                            "download_candidate": bool(
                                media.get("content_id") not in (None, "", "not_open")
                                and d.get("opened") is not False
                                and media.get("content_type") == "mp4"
                            ),
                            "reason": "Original availability is checked by download_video_original; not_open items cannot be downloaded",
                            **date_fields(d),
                        }
                    )
        return {"items": videos, "complete": True}

    async def download_video(self, course_id, item_id, allow_playback_copy=False):
        self.output_dir.mkdir(parents=True, exist_ok=True)
        destination = self.output_dir / f"video-{course_id}-{item_id}-{uuid4().hex[:8]}.mp4"
        result = await self.transport.video_original(course_id, item_id, destination, allow_playback_copy)
        if not result.get("available"):
            raise EClassError(result.get("reason", "Original video unavailable"))
        with destination.open("rb") as stream:
            prefix = stream.read(32)
            valid_mp4 = len(prefix) >= 12 and prefix[4:8] == b"ftyp"
            if valid_mp4:
                stream.seek(0)
                digest = hashlib.file_digest(stream, "sha256").hexdigest()
        if not valid_mp4:
            # Windows cannot unlink a file while its read handle is still open.
            destination.unlink(missing_ok=True)
            raise EClassError("Downloaded original is not an MP4 file")
        return {
            "path": str(destination),
            "bytes": destination.stat().st_size,
            "sha256": digest,
            "content_type": "video/mp4",
            "source": result["source"],
            "original": result["source"] == "commons_original",
            "course_id": course_id,
            "item_id": item_id,
            "attendance_recorded": False,
        }

    async def download(self, course_id, file_id):
        meta = await self.get(f"/api/v1/courses/{course_id}/files/{file_id}")
        if meta.get("locked_for_user"):
            raise EClassError("File is locked for this user")
        self.output_dir.mkdir(parents=True, exist_ok=True)
        name = Path(meta.get("display_name") or meta.get("filename") or f"file-{file_id}").name
        name = re.sub(r"[^\w.() -]", "_", name)[:150] or "file"
        destination = self.output_dir / f"{file_id}-{uuid4().hex[:8]}-{name}"
        result = await self.transport.download(
            f"/courses/{course_id}/files/{file_id}/download?wrap=1", destination, 64 * 1024 * 1024
        )
        size = destination.stat().st_size
        prefix = destination.read_bytes()[:512].lstrip().lower()
        if (
            not size
            or prefix.startswith((b"<!doctype html", b"<html"))
            or isinstance(meta.get("size"), int)
            and meta["size"] != size
        ):
            destination.unlink()
            raise EClassError("Downloaded file failed size/content validation")
        digest = hashlib.sha256(destination.read_bytes()).hexdigest()
        return {
            "path": str(destination),
            "bytes": destination.stat().st_size,
            "sha256": digest,
            "content_type": result["content_type"],
            "file_id": file_id,
        }

    @staticmethod
    def fingerprint(file):
        p = Path(file).expanduser().resolve()
        if not p.is_file() or p.stat().st_size > 10 * 1024 * 1024:
            raise EClassError("Each submission file must exist and be at most 10 MiB")
        return {
            "path": str(p),
            "name": p.name,
            "size": p.stat().st_size,
            "sha256": hashlib.sha256(p.read_bytes()).hexdigest(),
        }

    async def prepare(self, course_id, assignment_id, files):
        a = await self.assignment(course_id, assignment_id)
        user = (await self.session_status())["user"]
        existing = await self.submission(course_id, assignment_id)
        manifest = [self.fingerprint(f) for f in files]
        problems = []
        if not manifest:
            problems.append("At least one file is required")
        if "online_upload" not in a.get("submission_types", []):
            problems.append("Assignment does not allow file upload")
        if a.get("locked_for_user"):
            problems.append("Assignment is locked")
        allowed = a.get("allowed_extensions") or []
        if allowed and any(
            Path(f["name"]).suffix.lstrip(".").lower() not in [e.lower() for e in allowed] for f in manifest
        ):
            problems.append("File extension is not allowed")
        if a.get("group_category_id"):
            problems.append("Group submissions require a separately verified workflow")
        if a.get("turnitin_enabled") or a.get("vericite_enabled"):
            problems.append("External originality agreement requires a separately verified workflow")
        attempts = a.get("allowed_attempts", -1)
        if isinstance(attempts, int) and attempts >= 0 and (existing.get("attempt") or 0) >= attempts:
            problems.append("No submission attempts remain")
        if len(manifest) > 10 or sum(f["size"] for f in manifest) > 20 * 1024 * 1024:
            problems.append("Submission limit: 10 files and 20 MiB total")
        now = datetime.now(timezone.utc)
        self.plans = {
            key: value
            for key, value in self.plans.items()
            if datetime.fromisoformat(value["expires_at"]) > now
        }
        if len(self.plans) >= 100:
            raise EClassError("Too many pending plans; wait for expiry")
        plan_id = uuid4().hex
        plan = {
            "plan_id": plan_id,
            "course_id": course_id,
            "assignment_id": assignment_id,
            "assignment_name": a.get("name"),
            "user_id": user["id"],
            "files": manifest,
            "previous_attempt": existing.get("attempt"),
            "previous_submitted_at": existing.get("submitted_at"),
            "expires_at": (datetime.now(timezone.utc) + timedelta(minutes=10)).isoformat(),
            "problems": problems,
            "ready": not problems,
            "state": "prepared",
            "submission_enabled": self.enable_submission,
        }
        self.plans[plan_id] = plan
        return plan

    async def submit(self, plan_id, confirmation):
        async with self.submission_lock:
            return await self._submit(plan_id, confirmation)

    async def _submit(self, plan_id, confirmation):
        if not self.enable_submission:
            raise EClassError("Actual submission is disabled. No upload or POST was sent.")
        plan = self.plans.get(plan_id)
        if not plan or plan["state"] != "prepared" or not plan["ready"]:
            raise EClassError("A ready, unused preparation plan is required")
        if confirmation != f"SUBMIT {plan['assignment_id']}":
            raise EClassError("Confirmation must exactly match SUBMIT followed by the assignment ID")
        if datetime.fromisoformat(plan["expires_at"]) < datetime.now(timezone.utc):
            raise EClassError("Preparation plan expired")
        if [self.fingerprint(f["path"]) for f in plan["files"]] != plan["files"]:
            raise EClassError("Files changed after preparation")
        fresh = await self.prepare(
            plan["course_id"], plan["assignment_id"], [f["path"] for f in plan["files"]]
        )
        if (
            not fresh["ready"]
            or fresh["files"] != plan["files"]
            or fresh["user_id"] != plan["user_id"]
            or fresh["previous_attempt"] != plan["previous_attempt"]
            or fresh["previous_submitted_at"] != plan["previous_submitted_at"]
        ):
            raise EClassError("Assignment, session, or submission changed; prepare again")
        # Snapshot and validate the exact bytes sent. Adapters never reopen paths,
        # so edits during browser connection/upload cannot change approved content.
        payload = {"course": plan["course_id"], "assignment": plan["assignment_id"], "files": []}
        for entry in plan["files"]:
            try:
                with Path(entry["path"]).open("rb") as source:
                    content = source.read(10 * 1024 * 1024 + 1)
            except OSError:
                raise EClassError("Files changed or became unreadable after preparation") from None
            if len(content) != entry["size"] or hashlib.sha256(content).hexdigest() != entry["sha256"]:
                raise EClassError("Files changed after preparation")
            payload["files"].append(
                {
                    "name": entry["name"],
                    "type": mimetypes.guess_type(entry["name"])[0] or "application/octet-stream",
                    "data": base64.b64encode(content).decode(),
                }
            )
        # No automatic retries once a server write might have happened.
        plan["state"] = "in_flight"
        try:
            receipt = await self.transport.submit_files(payload)
            stored = await self.submission(plan["course_id"], plan["assignment_id"])
            expected = {str(i) for i in receipt["file_ids"]}
            actual = {str(f["id"]) for f in stored.get("attachments", [])}
            if not expected.issubset(actual) or not stored.get("submitted_at"):
                raise EClassError(
                    "Submission response could not be verified; check server state before retrying"
                )
            plan["state"] = "verified"
            return {"state": "verified", "submission": stored}
        except Exception:
            plan["state"] = "uncertain"
            raise EClassError(
                "Upload/submission outcome uncertain. Inspect get_submission; do not retry this plan."
            ) from None
