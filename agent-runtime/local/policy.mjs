import fs from 'node:fs/promises';
import path from 'node:path';

export class LocalError extends Error {
  constructor(code, message) { super(message); this.code = code; }
}
export const fail = (code, message) => { throw new LocalError(code, message); };
export const limits = { text: 1024 * 1024, entries: 2000, matches: 100, attachment: 20 * 1024 * 1024 };
export function inside(root, target) {
  const relative = path.relative(root, target);
  return relative === '' || (!relative.startsWith(`..${path.sep}`) && relative !== '..' && !path.isAbsolute(relative));
}
export function secretName(name) {
  const n = name.toLowerCase();
  return /^\.env(?:\.|$)/.test(n) || /\.(?:dpapi|pem|pfx|p12|key|age)$/.test(n)
    || /^(?:id_(?:rsa|ed25519|ecdsa)(?:\.pub)?|auth\.json|credentials(?:\.json)?|tokens\.json|secrets\.(?:yaml|yml|json)|\.npmrc|\.pypirc)$/.test(n);
}
export function protectedName(name) {
  return ['.homebase', '.ssh', '.aws', '.azure', '.gnupg', '.kube', '.age-keys', '.sandbox-secrets'].includes(name.toLowerCase());
}
export function normalizeRelative(value = '') {
  if (typeof value !== 'string' || value.length > 2048 || value.includes('\0') || path.isAbsolute(value)
    || /^[A-Za-z]:/.test(value) || /[:*?"<>|]/.test(value)) fail('PATH_INVALID', '请使用工作区内的相对路径');
  const segments = value.replaceAll('\\', '/').split('/').filter(p => p && p !== '.');
  if (segments.some(p => p === '..' || /[ .]$/.test(p) || /^(?:con|prn|aux|nul|com[0-9]|lpt[0-9])(?:\.|$)/i.test(p))) fail('PATH_INVALID', '路径包含越界或设备名称');
  if (segments.some(p => secretName(p) || protectedName(p))) fail('SECRET_PATH', '该路径属于凭据或受保护的配置，不通过助理文件工具读取');
  return segments;
}
export async function workspace(context, id, write = false) {
  const item = context.workspaces.find(w => w.id === (id || context.default_workspace));
  if (!item) fail('WORKSPACE_NOT_GRANTED', '目录尚未授权，请先选择或授权工作区');
  if (write && item.access !== 'write') fail('WORKSPACE_READ_ONLY', '该工作区只允许读取');
  if (!path.isAbsolute(item.path)) fail('WORKSPACE_INVALID', '工作区必须是绝对目录');
  if((await fs.lstat(item.path)).isSymbolicLink())fail('LINK_DENIED','工作区根目录已变成链接，请重新核对授权，未沿链接操作');
  const root = await fs.realpath(item.path);
  if (root === path.parse(root).root) fail('WORKSPACE_INVALID', '不能将整个磁盘作为助理工作区');
  for (const denied of context.protected_paths || []) {
    if (inside(path.resolve(denied), root) || inside(root, path.resolve(denied))) fail('PROTECTED_ROOT', '该授权覆盖受保护目录，请选择更具体的项目目录');
  }
  const info = await fs.lstat(root);
  if (!info.isDirectory() || info.isSymbolicLink()) fail('WORKSPACE_INVALID', '工作区不是普通目录');
  return { ...item, path: root };
}
export async function resolvePath(context, id, relative = '', { write = false, missing = false } = {}) {
  const root = await workspace(context, id, write);
  const segments = normalizeRelative(relative);
  let target = root.path;
  for (let i = 0; i < segments.length; i++) {
    target = path.join(target, segments[i]);
    let stat;
    try { stat = await fs.lstat(target); }
    catch (error) {
      if (missing && error.code === 'ENOENT') return { root, target: path.join(root.path, ...segments), relative: segments.join('/'), exists: false };
      throw error;
    }
    if (stat.isSymbolicLink()) fail('LINK_DENIED', '不沿符号链接或目录联接操作工作区外的数据');
    if (stat.isFile() && stat.nlink > 1) fail('HARDLINK_DENIED', '不读取或修改多重硬链接文件');
    if (!inside(root.path, await fs.realpath(target))) fail('PATH_ESCAPE', '解析后的路径超出工作区');
    if (i < segments.length - 1 && !stat.isDirectory()) fail('PATH_INVALID', '路径中间部分不是目录');
  }
  return { root, target, relative: segments.join('/'), exists: true };
}
export function safeFailure(error) {
  if (error instanceof LocalError) return { code: error.code, message: error.message };
  const messages = { ENOENT: '指定文件或目录不存在', EACCES: '没有文件访问权限', EPERM: '操作被系统拒绝', EEXIST: '目标已存在，未覆盖', ENOTEMPTY: '目录非空', ENOTDIR: '目标不是目录', EISDIR: '目标是目录', EXDEV: '跨磁盘移动不受支持，请先复制并核对' };
  return { code: messages[error.code] ? error.code : 'LOCAL_IO_FAILED', message: messages[error.code] || '本机操作未完成；未回显私人路径或原始错误' };
}
// A missing terminal/MCP completion is not proof that its side effects failed.
export function executionFailure(error) {
  const unknown = {
    EXECUTION_UNKNOWN: '隔离执行器未返回终态；结果未知，请核对原任务和产物，不自动重跑',
    MCP_CALL_UNKNOWN: 'MCP调用未确认完成；可能已被服务器接受，请核对原任务，不自动重试',
  };
  if (Object.hasOwn(unknown, error?.code)) return {status:'unknown',code:error.code,message:unknown[error.code]};
  return {status:'failed',...safeFailure(error)};
}
