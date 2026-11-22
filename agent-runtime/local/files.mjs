import fs from 'node:fs/promises';
import path from 'node:path';
import crypto from 'node:crypto';
import { TextDecoder } from 'node:util';
import { resolvePath, normalizeRelative, secretName, protectedName, limits, fail } from './policy.mjs';

const sha = data => crypto.createHash('sha256').update(data).digest('hex');
function textBytes(data) {
  try { return new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(data); }
  catch { fail('ENCODING_UNSUPPORTED', '文件不是有效 UTF-8；未转换原编码，请使用对应格式的能力处理'); }
}
async function readSmall(target) {
  const handle = await fs.open(target, 'r');
  try {
    const stat = await handle.stat();
    if (!stat.isFile()) fail('NOT_FILE', '目标不是普通文件');
    if (stat.nlink > 1) fail('HARDLINK_DENIED', '不读取多重硬链接文件');
    if (stat.size > limits.text) fail('FILE_TOO_LARGE', '文本超过 1MiB，请缩小文件或使用专用处理能力');
    const data = await handle.readFile();
    if (data.includes(0)) fail('BINARY_FILE', '这是二进制文件；可以查看元数据或导出附件，不作为空文本返回');
    return { data, text: textBytes(data), sha256: sha(data), stat };
  } finally { await handle.close(); }
}
async function rows(context, id, relative, count = limits.entries) {
  const location = await resolvePath(context, id, relative);
  const names = await fs.readdir(location.target, { withFileTypes: true });
  const result = []; const issues = [];
  for (const entry of names.sort((a, b) => a.name.localeCompare(b.name))) {
    if (result.length >= count) break;
    if (secretName(entry.name) || protectedName(entry.name)) continue;
    const child = [location.relative, entry.name].filter(Boolean).join('/');
    try {
      const resolved = await resolvePath(context, id, child);
      const stat = await fs.stat(resolved.target);
      result.push({ path: child, name: entry.name, kind: stat.isDirectory() ? 'directory' : stat.isFile() ? 'file' : 'unsupported', bytes: stat.size, modified_at: stat.mtime.toISOString() });
    } catch (e) { issues.push({ path: child, code: e.code || 'UNREADABLE' }); }
  }
  return { entries: result, issues, truncated: names.length > count };
}
async function publish(context, id, relative, data, expected, createOnly) {
  let loc = await resolvePath(context, id, relative, { write: true, missing: true });
  if (!loc.relative) fail('PATH_INVALID', '不能替换工作区根目录');
  if (loc.exists) {
    if (createOnly) fail('EEXIST', '目标已存在，未覆盖');
    if (!expected) fail('EXPECTED_HASH_REQUIRED', '编辑已有文件需要先读取并提供原 SHA-256');
    if ((await readSmall(loc.target)).sha256 !== expected) fail('FILE_CONFLICT', '文件已被其他操作修改，未覆盖；请重新读取后编辑');
  } else if (expected) fail('FILE_CONFLICT', '原文件已不存在，未创建替代文件');
  const parent = path.posix.dirname(loc.relative);
  await resolvePath(context, id, parent === '.' ? '' : parent, { write: true });
  const temp = path.join(path.dirname(loc.target), `.hb-write-${crypto.randomUUID()}.tmp`);
  await fs.writeFile(temp, data, { flag: 'wx' });
  let published = false;
  try {
    loc = await resolvePath(context, id, relative, { write: true, missing: true });
    if (createOnly) {
      // link provides atomic no-clobber publication; remove only our temporary link.
      await fs.link(temp, loc.target);
    } else {
      if (!loc.exists || (await readSmall(loc.target)).sha256 !== expected) fail('FILE_CONFLICT', '发布前文件发生变化，未覆盖');
      await fs.rename(temp, loc.target);
      published = true;
    }
    return { status: 'succeeded', path: loc.relative, bytes: data.length, sha256: sha(data) };
  } finally { if (!published) await fs.unlink(temp).catch(() => {}); }
}
export async function fileOperation(context, request) {
  const { action, workspace_id: id, path: relative = '' } = request;
  if (action === 'stat' || action === 'export') {
    const loc = await resolvePath(context, id, relative);
    const stat = await fs.stat(loc.target);
    if (stat.isDirectory()) return { status: 'succeeded', path: loc.relative, kind: 'directory', modified_at: stat.mtime.toISOString() };
    if (!stat.isFile() || stat.size > limits.attachment) fail('FILE_TOO_LARGE', '仅支持不超过20MiB的普通文件');
    const data = await fs.readFile(loc.target);
    const result = { path: loc.relative, kind: 'file', bytes: data.length, sha256: sha(data), modified_at: stat.mtime.toISOString() };
    if (action === 'stat') return { status: 'succeeded', ...result };
    if (!context.output_root || !path.isAbsolute(context.output_root)) fail('OUTPUT_INVALID', '受控产物目录尚未配置');
    const directory = path.join(context.output_root, crypto.randomUUID());
    await fs.mkdir(directory, { recursive: true });
    const output = path.join(directory, path.basename(loc.target));
    await fs.writeFile(output, data, { flag: 'wx' });
    const types = { '.txt': 'text/plain', '.md': 'text/markdown', '.json': 'application/json', '.csv': 'text/csv', '.mp4': 'video/mp4', '.png': 'image/png', '.jpg': 'image/jpeg', '.pdf': 'application/pdf', '.html': 'text/html' };
    return { status: 'succeeded', artifact: { path: output, bytes: data.length, sha256: result.sha256, mime_type: types[path.extname(output).toLowerCase()] || 'application/octet-stream' } };
  }
  if (action === 'list') {
    const offset=request.offset??0,limit=request.limit??20;
    if(!Number.isInteger(offset)||offset<0||!Number.isInteger(limit)||limit<1||limit>100)fail('PAGING_INVALID','目录分页大小须为1至100');
    const result=await rows(context,id,relative);
    return {status:'succeeded',...result,entries:result.entries.slice(offset,offset+limit),offset,total:result.entries.length,next_offset:offset+limit<result.entries.length?offset+limit:null};
  }
  if (action === 'read') {
    const loc = await resolvePath(context, id, relative);
    const result = await readSmall(loc.target);
    const offset = request.offset ?? 0, limit = request.limit ?? 200;
    if (!Number.isInteger(offset) || offset < 0 || !Number.isInteger(limit) || limit < 1 || limit > 500) fail('PAGING_INVALID', '行偏移或分页大小无效');
    const lines = result.text.split(/\r?\n/);
    const charOffset=request.char_offset??0,charLimit=request.char_limit??1800;
    if(!Number.isInteger(charOffset)||charOffset<0||!Number.isInteger(charLimit)||charLimit<1||charLimit>2000)fail('PAGING_INVALID','字符分页参数无效，单页至多2000字符');
    const selected=Array.from(lines.slice(offset,offset+limit).join('\n'));
    if(charOffset>selected.length)fail('PAGING_INVALID','字符偏移超出此行范围，请沿用上一页返回的next');
    const moreChars=charOffset+charLimit<selected.length;
    const next=moreChars?{offset,limit,char_offset:charOffset+charLimit}:offset+limit<lines.length?{offset:offset+limit,limit,char_offset:0}:null;
    return { status: 'succeeded', path: loc.relative, content: selected.slice(charOffset,charOffset+charLimit).join(''), offset, char_offset:charOffset, lines: lines.length, empty:result.data.length===0, next, next_offset:next?.offset??null, next_char_offset:next?.char_offset??null, sha256: result.sha256, encoding: result.data.subarray(0, 3).equals(Buffer.from([239, 187, 191])) ? 'utf-8-bom' : 'utf-8', newline: result.text.includes('\r\n') ? 'crlf' : 'lf' };
  }
  if (action === 'write') {
    if (typeof request.content !== 'string' || Buffer.byteLength(request.content) > limits.text) fail('CONTENT_INVALID', '新文件需要不超过 1MiB 的文本');
    if (request.expected_sha256) fail('USE_EXACT_EDIT', '修改已有文本请用 edit 精确替换，保留原编码和换行');
    return publish(context, id, relative, Buffer.from(request.content), undefined, true);
  }
  if (action === 'edit') {
    if (typeof request.old_text !== 'string' || !request.old_text || typeof request.new_text !== 'string') fail('EDIT_INVALID', '精确编辑需要非空 old_text 与 new_text');
    const loc = await resolvePath(context, id, relative, { write: true });
    const old = await readSmall(loc.target);
    if (old.sha256 !== request.expected_sha256) fail('FILE_CONFLICT', '文件版本不一致，未编辑');
    const oldText = old.text.includes('\r\n') ? request.old_text.replace(/\r?\n/g, '\r\n') : request.old_text;
    const replacement = old.text.includes('\r\n') ? request.new_text.replace(/\r?\n/g, '\r\n') : request.new_text;
    const at = old.text.indexOf(oldText);
    if (at < 0 || old.text.indexOf(oldText, at + oldText.length) >= 0) fail('EDIT_AMBIGUOUS', '原片段未找到或不唯一，请提供更具体的片段');
    const next = Buffer.from(old.text.slice(0, at) + replacement + old.text.slice(at + oldText.length));
    if (next.length > limits.text) fail('FILE_TOO_LARGE', '编辑后文本超过 1MiB');
    return publish(context, id, relative, next, old.sha256, false);
  }
  if (action === 'mkdir') {
    const loc = await resolvePath(context, id, relative, { write: true, missing: true });
    if (!loc.relative || loc.exists) fail('EEXIST', '目标已存在，未修改');
    const segments = normalizeRelative(relative);
    let directory = '';
    for (const segment of segments) {
      directory = [directory, segment].filter(Boolean).join('/');
      const next = await resolvePath(context, id, directory, { write: true, missing: true });
      if (!next.exists) await fs.mkdir(next.target);
    }
    return { status: 'succeeded', path: loc.relative };
  }
  if (action === 'search') {
    if (typeof request.query !== 'string' || !request.query || request.query.length > 500) fail('QUERY_INVALID', '搜索需要非空的简短文字');
    const pending = [relative], matches = [], issues = []; let visited = 0, truncated = false;
    while (pending.length && visited < limits.entries && matches.length < limits.matches) {
      const listing = await rows(context, id, pending.shift());
      truncated ||= listing.truncated;
      issues.push(...listing.issues);
      for (const entry of listing.entries) {
        if (++visited > limits.entries) break;
        if (entry.kind === 'directory') { if (!['.git', 'node_modules', 'target', '.cache', '.homebase-trash'].includes(entry.name)) pending.push(entry.path); continue; }
        if (entry.path.toLowerCase().includes(request.query.toLowerCase())) matches.push({ path: entry.path, kind: 'filename' });
        if ((request.search_content === true || request.content === true) && entry.bytes <= limits.text) {
          try {
            const file = await resolvePath(context, id, entry.path);
            const body = await readSmall(file.target);
            for (const [index, line] of body.text.split(/\r?\n/).entries()) if (line.includes(request.query)) { matches.push({ path: entry.path, line: index + 1, excerpt: line.slice(0, 300) }); if (matches.length >= limits.matches) break; }
          } catch (e) { if (!['BINARY_FILE', 'ENCODING_UNSUPPORTED'].includes(e.code)) issues.push({ path: entry.path, code: e.code || 'UNREADABLE' }); }
        }
        if (matches.length >= limits.matches) break;
      }
    }
    return { status: 'succeeded', matches: matches.slice(0, limits.matches), issues, visited, truncated: truncated || pending.length > 0 || visited >= limits.entries || matches.length >= limits.matches };
  }
  if (action === 'copy' || action === 'move') {
    const source = await resolvePath(context, id, relative, { write: action === 'move' });
    const destination = await resolvePath(context, request.destination_workspace_id || id, request.destination, { write: true, missing: true });
    if (!source.relative || !destination.relative || destination.exists) fail('DESTINATION_INVALID', '源或目标无效，不能覆盖已有目标');
    const stat = await fs.stat(source.target);
    if (!stat.isFile()) fail('DIRECTORY_MOVE_REQUIRES_REVIEW', '请逐项处理目录内容，当前复制和移动只接受普通文件');
    await resolvePath(context, destination.root.id, path.posix.dirname(destination.relative) === '.' ? '' : path.posix.dirname(destination.relative), { write: true });
    if (action === 'copy') await fs.copyFile(source.target, destination.target, 1);
    else {
      // Hard-link + unlink gives atomic no-clobber publication on the same volume.
      await fs.link(source.target, destination.target);
      await fs.unlink(source.target);
    }
    return { status: 'succeeded', path: source.relative, destination: destination.relative };
  }
  if (action === 'trash') {
    const loc = await resolvePath(context, id, relative, { write: true });
    if (!loc.relative || loc.relative.startsWith('.homebase-trash')) fail('PATH_INVALID', '不能回收工作区根目录或回收区');
    const stat = await fs.stat(loc.target);
    if (!stat.isFile()) fail('NOT_FILE', '当前回收操作只接受普通文件');
    const trash = await resolvePath(context, id, '.homebase-trash', { write: true, missing: true });
    if (!trash.exists) await fs.mkdir(trash.target);
    const destination = `.homebase-trash/${crypto.randomUUID()}-${path.basename(loc.target)}`;
    return { ...await fileOperation(context, { action: 'move', workspace_id: id, path: loc.relative, destination }), recoverable: true };
  }
  fail('ACTION_UNSUPPORTED', '文件操作尚未支持，请查看工具说明');
}
