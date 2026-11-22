/**
 * MarkdownRenderer — 安全只读 markdown 渲染（Phase 14.3）
 *
 * 设计原则：
 * - **安全 (security-first)**：rehype-sanitize 默认 schema + 链接强制 target=_blank rel=noopener
 * - **样式**：@tailwindcss/typography `prose` 类 + dark:prose-invert，与 GDNbit 字体栈对齐
 * - **GFM**：remark-gfm 支持表格 / 任务列表 / 删除线
 * - **图片占位**：不渲染外部 img（避免远程加载 / 隐私），用 `[图片: alt]` 文本替代
 * - **代码块**：暂不做语法高亮（5 MB 文件高亮会拖慢；后续可加 shiki）
 *
 * 用法：
 * ```tsx
 * <MarkdownRenderer source={fileContent} />
 * ```
 */

import ReactMarkdown from "react-markdown";
import { memo } from "react";
import remarkGfm from "remark-gfm";
import rehypeSanitize from "rehype-sanitize";
import { cn } from "@/lib/utils";

interface MarkdownRendererProps {
  source: string;
  /** 附加 className（默认应用 prose 样式） */
  className?: string;
}

export const MarkdownRenderer = memo(function MarkdownRenderer({ source, className }: MarkdownRendererProps) {
  return (
    <div
      className={cn(
        // 基础 prose + 暗色翻转
        "prose prose-slate max-w-none dark:prose-invert",
        // 字体栈对齐 GDNbit（serif 标题 + sans 正文 + mono 代码）
        "prose-headings:font-serif prose-headings:font-semibold",
        "prose-code:font-mono prose-code:text-[0.9em] prose-code:before:hidden prose-code:after:hidden",
        "prose-pre:bg-muted prose-pre:text-foreground",
        // 链接颜色（GDNbit primary 色板）
        "prose-a:text-primary-700 hover:prose-a:text-primary-800 dark:prose-a:text-primary-400 dark:hover:prose-a:text-primary-300",
        // 表格细线
        "prose-table:border prose-table:border-border prose-th:border prose-th:border-border prose-th:bg-muted/40 prose-th:px-3 prose-th:py-2 prose-td:border prose-td:border-border prose-td:px-3 prose-td:py-2",
        // 引用块淡色
        "prose-blockquote:border-l-primary-500/40 prose-blockquote:bg-muted/30 prose-blockquote:py-1 prose-blockquote:not-italic",
        className,
      )}
    >
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        rehypePlugins={[rehypeSanitize]}
        components={{
          // 链接强制新窗口 + 安全 rel
          a: ({ children, href, ...rest }) => (
            <a href={href} target="_blank" rel="noopener noreferrer" {...rest}>
              {children}
            </a>
          ),
          // 不加载外部图片：用 alt 文本占位（隐私 / 防止外部资源加载错误）
          img: ({ alt }) => (
            <span className="my-2 inline-block rounded border border-dashed border-border bg-muted/40 px-2 py-1 font-mono text-xs text-muted-foreground">
              [图片: {alt || "未提供 alt"}]
            </span>
          ),
        }}
      >
        {source}
      </ReactMarkdown>
    </div>
  );
});
