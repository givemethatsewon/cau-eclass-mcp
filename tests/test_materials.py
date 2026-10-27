import copy
import hashlib
from io import BytesIO

import pytest
from fastmcp import Client
from pypdf import PdfWriter
from pypdf.generic import DecodedStreamObject, DictionaryObject, NameObject

from eclass_mcp.server import create_server
from eclass_mcp.service import EClassService
from eclass_mcp.transport import EClassError


def pdf_document(*texts):
    writer = PdfWriter()
    for value in texts:
        page = writer.add_blank_page(width=600, height=800)
        if value:
            font = DictionaryObject({
                NameObject('/Type'): NameObject('/Font'),
                NameObject('/Subtype'): NameObject('/Type1'),
                NameObject('/BaseFont'): NameObject('/Helvetica'),
            })
            page[NameObject('/Resources')] = DictionaryObject({
                NameObject('/Font'): DictionaryObject({NameObject('/F1'): writer._add_object(font)})
            })
            stream = DecodedStreamObject()
            stream.set_data(f'BT /F1 12 Tf 50 700 Td ({value}) Tj ET'.encode())
            page[NameObject('/Contents')] = writer._add_object(stream)
    output = BytesIO()
    writer.write(output)
    return output.getvalue()


class MaterialTransport:
    def __init__(self, data=None):
        self.data = data or pdf_document('Alpha', 'Beta', '')
        self.downloads = 0
        self.locked = False
        self.revision = '2026-10-01T00:00:00Z'
        self.files_denied = False

    def modules(self):
        return [{
            'module_id': 7, 'title': '9주차', 'position': 9,
            'module_items': [{
                'module_item_id': 8, 'title': 'Scheduler', 'content_type': 'attendance_item',
                'content_id': 22, 'content_data': {
                    'item_id': 22, 'course_id': 11, 'item_content_type': 'commons',
                    'opened': not self.locked, 'lecture_period_status': 'open',
                    'updated_at': self.revision,
                    'item_content_data': {
                        'content_id': 'synthetic', 'content_type': 'pdf',
                        'file_name': 'Scheduler.pdf', 'view_url': 'https://ocs.cau.ac.kr/em/synthetic',
                    },
                },
            }],
        }]

    async def get(self, path, **kwargs):
        if path == '/learningx/api/v1/courses/11/modules?include_detail=true':
            return {'body': copy.deepcopy(self.modules())}
        if path == '/api/v1/courses/11/files?per_page=100':
            if self.files_denied:
                raise EClassError('e-Class HTTP 403')
            return {'body': [self.canvas_meta()], 'link': None}
        if path == '/api/v1/courses/11/files/33':
            return {'body': self.canvas_meta()}
        if path.startswith('/api/v1/courses?'):
            return {'body': [
                {'id': 11, 'name': '리눅스 시스템', 'term': {'id': 96, 'name': '2026년 2학기', 'unused': True}},
                {'id': 12, 'name': '리눅스 시스템', 'term': {'id': 93, 'name': '2026년 1학기'}},
            ], 'link': None}
        raise EClassError('Unknown course or file')

    def canvas_meta(self):
        return {'id': 33, 'display_name': 'Notes.pdf', 'content-type': 'application/pdf',
                'size': len(self.data), 'locked_for_user': self.locked, 'updated_at': self.revision}

    async def commons_pdf(self, course_id, item_id, destination, max_bytes=64 * 1024 * 1024):
        assert (course_id, item_id) == (11, 22)
        self.downloads += 1
        destination.write_bytes(self.data)
        return {'available': True, 'source': 'commons_original', 'original': True,
                'title': 'Scheduler', 'bytes': len(self.data), 'content_type': 'application/pdf'}

    async def download(self, path, destination, max_bytes):
        assert path == '/courses/11/files/33/download?wrap=1'
        self.downloads += 1
        destination.write_bytes(self.data)
        return {'content_type': 'application/pdf'}


async def test_unified_commons_download_and_read_reuse_verified_pdf(tmp_path):
    transport = MaterialTransport()
    service = EClassService(transport, output_dir=tmp_path)
    downloaded = await service.material_download('commons:11:22')
    assert downloaded['sha256'] == hashlib.sha256(transport.data).hexdigest()
    assert downloaded['bytes'] == len(transport.data)
    assert downloaded['page_count'] == 3 and downloaded['original'] is True
    result = await service.material_read('commons:11:22')
    assert [page['text'].strip() for page in result['pages']] == ['Alpha', 'Beta', '']
    assert result['pages_without_text'] == [3] and result['text_complete'] is False
    assert result['path'] == downloaded['path'] and transport.downloads == 1


async def test_canvas_and_commons_use_the_same_resource_api(tmp_path):
    transport = MaterialTransport(pdf_document('Canvas notes'))
    result = await EClassService(transport, output_dir=tmp_path).material_read('canvas:11:33')
    assert result['pages'][0]['text'].strip() == 'Canvas notes'
    assert result['resource_id'] == 'canvas:11:33' and result['source'] == 'canvas_file'
    assert result['text_complete'] is True and result['document_complete'] is True


@pytest.mark.parametrize('data', [b'<html>login</html>', b'%PDF-invalid'])
async def test_invalid_material_pdf_is_removed(tmp_path, data):
    with pytest.raises(EClassError, match='PDF'):
        await EClassService(MaterialTransport(data), output_dir=tmp_path).material_download('commons:11:22')
    assert not list(tmp_path.iterdir())


async def test_cached_material_still_rechecks_access(tmp_path):
    transport = MaterialTransport()
    service = EClassService(transport, output_dir=tmp_path)
    await service.material_download('commons:11:22')
    transport.locked = True
    with pytest.raises(EClassError):
        await service.material_read('commons:11:22')
    assert transport.downloads == 1


async def test_material_revision_invalidates_cached_pdf(tmp_path):
    transport = MaterialTransport()
    service = EClassService(transport, output_dir=tmp_path)
    old = await service.material_download('commons:11:22')
    transport.revision = '2026-10-02T00:00:00Z'
    transport.data = pdf_document('Revised')
    new = await service.material_read('commons:11:22')
    assert new['path'] != old['path'] and new['pages'][0]['text'].strip() == 'Revised'
    assert transport.downloads == 2


async def test_pdf_character_budget_can_resume_inside_a_page_without_losing_text(tmp_path):
    service = EClassService(MaterialTransport(pdf_document('ABCDEFGHIJKLMNOPQRSTUVWXYZ')), output_dir=tmp_path)
    pieces, page, offset = [], 1, 0
    while page is not None:
        result = await service.material_read('commons:11:22', start_page=page, start_offset=offset, max_chars=8)
        pieces.extend(value['text'] for value in result['pages'])
        page, offset = result['next_page'], result['next_offset']
    assert ''.join(pieces) == 'ABCDEFGHIJKLMNOPQRSTUVWXYZ'


async def test_pdf_range_validation_does_not_silently_read_another_page(tmp_path):
    service = EClassService(MaterialTransport(), output_dir=tmp_path)
    with pytest.raises(EClassError, match='page'):
        await service.material_read('commons:11:22', start_page=4)
    with pytest.raises(EClassError, match='offset'):
        await service.material_read('commons:11:22', start_offset=999)


async def test_bounded_pdf_text_does_not_reveal_a_signed_url_split_across_chunks(tmp_path):
    service = EClassService(MaterialTransport(pdf_document('https://example.com/a?token=PRIVATE')), output_dir=tmp_path)
    pieces, page, offset = [], 1, 0
    while page is not None:
        result = await service.material_read('commons:11:22', start_page=page, start_offset=offset, max_chars=8)
        pieces.extend(value['text'] for value in result['pages'])
        page, offset = result['next_page'], result['next_offset']
    assert 'PRIVATE' not in ''.join(pieces)


@pytest.mark.parametrize('budget', [8, 22, 23, 31])
async def test_mcp_page_chunks_are_sanitized_once_and_resume_without_corruption(tmp_path, budget):
    transport = MaterialTransport(pdf_document('https://example.com/a?token=PRIVATE'))
    original = transport.modules

    def private_title():
        modules = original()
        modules[0]['module_items'][0]['title'] = 'https://example.com/a?token=PRIVATE'
        return modules

    transport.modules = private_title
    async with Client(create_server(EClassService(transport, output_dir=tmp_path))) as client:
        pieces, page, offset = [], 1, 0
        while page is not None:
            response = await client.call_tool('read_material', {'resource_id': 'commons:11:22',
                'start_page': page, 'start_offset': offset, 'max_chars': budget})
            result = response.data['result']
            assert 'PRIVATE' not in result['title']
            assert sum(len(value['text']) for value in result['pages']) <= budget
            pieces.extend(value['text'] for value in result['pages'])
            page, offset = result['next_page'], result['next_offset']
    assert ''.join(pieces) == 'https://example.com/a?token=%5BREDACTED%5D'


async def test_material_filters_and_pagination_preserve_completeness(tmp_path):
    service = EClassService(MaterialTransport(), output_dir=tmp_path)
    first = await service.materials(11, limit=1)
    assert len(first['items']) == 1 and first['has_more'] and first['next_offset'] == 1
    second = await service.materials(11, offset=first['next_offset'], limit=1)
    assert len(second['items']) == 1 and not second['has_more'] and second['complete']
    result = await service.materials(11, week=9, query='scheduleR', content_type='application/pdf')
    assert [item['resource_id'] for item in result['items']] == ['commons:11:22']


async def test_canvas_permission_error_does_not_hide_accessible_weekly_materials(tmp_path):
    transport = MaterialTransport()
    transport.files_denied = True
    result = await EClassService(transport, output_dir=tmp_path).materials(11)
    assert [item['resource_id'] for item in result['items']] == ['commons:11:22']
    assert result['complete'] is False and result['warnings']


async def test_course_filter_uses_term_and_compact_term_metadata():
    result = await EClassService(MaterialTransport()).courses(query='리눅스시스템', term='2026년 2학기')
    assert [course['id'] for course in result['items']] == [11]
    assert result['items'][0]['term'] == {'id': 96, 'name': '2026년 2학기'}


async def test_legacy_detail_filter_preserves_only_matching_raw_items():
    transport = MaterialTransport()
    original = transport.modules

    def with_unrelated_item():
        modules = original()
        modules[0]['module_items'].append({'module_item_id': 9, 'title': 'Assignment',
                                           'content_type': 'assignment'})
        return modules

    transport.modules = with_unrelated_item
    result = await EClassService(transport).learning_items(11, query='Scheduler', detail=True)
    assert [item['title'] for item in result['items'][0]['module_items']] == ['Scheduler']
    assert result['items'][0]['module_items'][0]['content_data']['item_id'] == 22


async def test_material_tools_expose_unified_ids_and_bounded_reading(tmp_path):
    async with Client(create_server(EClassService(MaterialTransport(), output_dir=tmp_path))) as client:
        result = await client.call_tool('list_materials', {'course_id': 11, 'week': 9})
        resource_id = result.data['result']['items'][0]['resource_id']
        result = await client.call_tool('read_material', {'resource_id': resource_id, 'start_page': 2, 'end_page': 2})
        assert result.data['result']['pages'][0]['text'].strip() == 'Beta'
        with pytest.raises(Exception, match='greater than'):
            await client.call_tool('read_material', {'resource_id': resource_id, 'start_page': 0})
