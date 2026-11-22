/**
 * AIKnowledgeGraph — 本机磁盘整体架构图（React Flow）
 *
 * Phase 14.5b 用户需求重定位：
 * - 节点 = 用户磁盘的顶层文件夹（"本机操作指南" / "C:\HomeBase\Wiki" / "C:\HomeBase\Projects"）
 * - 单击节点 → 懒加载 list_dir，展开下一层子文件夹（中文 label override 来自 topology.json）
 * - 节点右上角 ExternalLink 图标 → topology_open_dir（OS explorer.exe 打开）
 * - 节点之间连线：
 *   - 用户配置的 topology.json `edges` 数组（实线 + label）
 *   - 解析自节点路径下 INDEX.md / README.md frontmatter 的 references（虚线 + label）
 *
 * 数据源：topologyApi.load / listDir / parseRefs
 *
 * 视觉层级（用户原话 "从粗到深"）：
 * - root 节点：大字、品牌色边框、note 副行
 * - 子节点：小字、灰边、文件夹图标
 */

import { useCallback, useEffect, useMemo, useState } from "react";
import {
  ReactFlow,
  Background,
  Controls,
  Handle,
  MarkerType,
  Position,
  type Edge,
  type Node,
  type NodeProps,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { useQuery, useQueries } from "@tanstack/react-query";
import {
  ChevronDown,
  ChevronRight,
  Code2,
  ExternalLink,
  FileText,
  Film,
  Folder,
  FolderOpen,
  Gamepad2,
  Image as ImageIcon,
  Loader2,
  Maximize2,
  Minimize2,
  Package,
  Settings2,
  type LucideIcon,
} from "lucide-react";
import {
  topologyApi,
  type Topology,
  type TopologyDirEntry,
  type TopologyRefEdge,
  type TopologyRoot,
  type VirtualGroup,
} from "@/lib/tauri-api";
import { cn } from "@/lib/utils";

// =====================================================================
// 虚拟组的 lucide 图标映射（方案 A v2）
// =====================================================================

/**
 * 把 topology.json 里填的 `icon: "film"` 映射成 lucide 组件
 *
 * 常用语义：`film` 影视 · `image` 图像 · `package` 软件 · `code` 开发 ·
 * `gamepad-2` 游戏 · `folder-open` 其他 · `folder` 默认
 */
function iconForGroup(icon: string | null | undefined): LucideIcon {
  switch ((icon ?? "folder").toLowerCase()) {
    case "film":
    case "video":
      return Film;
    case "image":
    case "photo":
    case "picture":
      return ImageIcon;
    case "package":
    case "software":
    case "app":
      return Package;
    case "code":
    case "code2":
    case "dev":
      return Code2;
    case "gamepad":
    case "gamepad-2":
    case "game":
      return Gamepad2;
    case "folder-open":
    case "other":
      return FolderOpen;
    default:
      return Folder;
  }
}

// =====================================================================
// 主组件
// =====================================================================

export function AIKnowledgeGraph() {
  // 1. 加载 topology
  const topo = useQuery({
    queryKey: ["topology", "load"],
    queryFn: () => topologyApi.load(),
  });

  // 2. 用户展开的 root.id 集合（懒加载子节点）
  const [expanded, setExpanded] = useState<Set<string>>(new Set());

  // 全屏模式（Phase 14.5c 新增，应用户要求）
  const [isFullscreen, setIsFullscreen] = useState(false);

  // ESC 键退出全屏
  useEffect(() => {
    if (!isFullscreen) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") setIsFullscreen(false);
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [isFullscreen]);

  // 3. 每个展开 root 异步拉子目录
  //    v2：配了虚拟组的 drive 展开时显示 groups，不需要 list_dir
  const driveGroups = topo.data?.drive_groups ?? {};
  const childQueries = useQueries({
    queries: (topo.data?.roots ?? [])
      .filter((r) => expanded.has(r.id) && r.exists)
      .filter((r) => !(driveGroups[r.id]?.length > 0))
      .map((r) => ({
        queryKey: ["topology", "list", r.absolute_path],
        queryFn: () => topologyApi.listDir(r.absolute_path),
      })),
  });

  // 4. 每个 root 拉一次 references（用于跨节点连线）
  const refQueries = useQueries({
    queries: (topo.data?.roots ?? [])
      .filter((r) => r.exists)
      .map((r) => ({
        queryKey: ["topology", "refs", r.absolute_path],
        queryFn: () => topologyApi.parseRefs(r.absolute_path),
      })),
  });

  const toggleExpand = useCallback((rootId: string) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(rootId)) next.delete(rootId);
      else next.add(rootId);
      return next;
    });
  }, []);

  const handleOpenDir = useCallback(async (absolutePath: string) => {
    try {
      await topologyApi.openDir(absolutePath);
    } catch (e) {
      console.error("topology_open_dir 失败：", e);
    }
  }, []);

  // 5. 构建 React Flow 节点 + 边
  const { nodes, edges } = useMemo(() => {
    if (!topo.data) return { nodes: [], edges: [] };

    // 把 child queries 收回成 Map<rootId, entries[]>
    // ⚠️ 过滤条件必须与 childQueries 里的一致（否则索引错位）
    const childMap: Record<string, TopologyDirEntry[]> = {};
    const rootsWithListDir = topo.data.roots
      .filter((r) => expanded.has(r.id) && r.exists)
      .filter((r) => !(topo.data!.drive_groups?.[r.id]?.length > 0));
    rootsWithListDir.forEach((r, i) => {
      const q = childQueries[i];
      if (q?.data) childMap[r.id] = q.data;
    });

    // 收集 reference edges
    const allRefs: TopologyRefEdge[] = [];
    refQueries.forEach((q) => {
      if (q.data) allRefs.push(...q.data);
    });

    return buildGraph({
      topology: topo.data,
      expanded,
      childMap,
      refs: allRefs,
      onToggleExpand: toggleExpand,
      onOpenDir: handleOpenDir,
    });
  }, [topo.data, expanded, childQueries, refQueries, toggleExpand, handleOpenDir]);

  const nodeTypes = useMemo(
    () => ({
      root: RootNode,
      child: ChildNode,
      group: GroupNode,
    }),
    [],
  );

  return (
    <div
      className={cn(
        "rounded-xl border border-border bg-card",
        isFullscreen &&
          "fixed inset-0 z-50 rounded-none border-0 bg-background shadow-2xl",
      )}
    >
      <div className="flex items-center justify-between border-b border-border/60 px-4 py-3">
        <div>
          <h3 className="font-serif text-base font-semibold">📊 本机磁盘架构图</h3>
          <p className="mt-0.5 text-xs text-muted-foreground">
            拖动左侧彩条移动节点 · 点节点展开下一层 · 右上角 <ExternalLink className="inline h-3 w-3" /> 用 explorer 打开
            {topo.data?.source === "user_config" ? " · 来自 topology.json" : " · 默认配置"}
          </p>
        </div>
        <div className="flex items-center gap-2">
          <ConfigInfo topology={topo.data} />
          <button
            type="button"
            onClick={() => setIsFullscreen((v) => !v)}
            className="inline-flex items-center gap-1 rounded-md border border-border bg-card px-2 py-1 text-[11px] text-muted-foreground transition-colors hover:border-primary-400 hover:text-foreground"
            title={isFullscreen ? "退出全屏（或按 ESC）" : "全屏查看"}
          >
            {isFullscreen ? (
              <>
                <Minimize2 className="h-3 w-3" />
                退出全屏
              </>
            ) : (
              <>
                <Maximize2 className="h-3 w-3" />
                全屏
              </>
            )}
          </button>
        </div>
      </div>
      <div style={{ width: "100%", height: isFullscreen ? "calc(100vh - 57px)" : 520 }}>
        {topo.isLoading ? (
          <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
            <Loader2 className="mr-2 h-4 w-4 animate-spin" />
            加载磁盘 topology…
          </div>
        ) : (
          <ReactFlow
            nodes={nodes}
            edges={edges}
            nodeTypes={nodeTypes}
            fitView
            fitViewOptions={{ padding: 0.2 }}
            proOptions={{ hideAttribution: true }}
            nodesDraggable={true}
            nodesConnectable={false}
            edgesFocusable={false}
            panOnDrag
            zoomOnScroll={false}
            zoomOnPinch
            minZoom={0.4}
            maxZoom={1.6}
          >
            <Background gap={16} size={1} />
            <Controls showInteractive={false} />
          </ReactFlow>
        )}
      </div>
    </div>
  );
}

// =====================================================================
// ConfigInfo — 顶部右侧的「编辑配置」提示
// =====================================================================

function ConfigInfo({ topology }: { topology: Topology | undefined }) {
  if (!topology) return null;
  const isDefault = topology.source === "default";
  return (
    <div
      className="flex items-center gap-1.5 text-[11px] text-muted-foreground"
      title={
        isDefault
          ? `创建 ${topology.config_path} 自定义节点`
          : `读自 ${topology.config_path}`
      }
    >
      <Settings2 className="h-3 w-3" />
      <code className="font-mono">{shorten(topology.config_path)}</code>
    </div>
  );
}

function shorten(p: string): string {
  if (!p) return "";
  // 只显示后两段
  const segs = p.split(/[\\/]+/);
  if (segs.length <= 3) return p;
  return `…\\${segs.slice(-2).join("\\")}`;
}

// =====================================================================
// 节点组件
// =====================================================================

interface RootNodeData extends Record<string, unknown> {
  root: TopologyRoot;
  expanded: boolean;
  onToggle: () => void;
  onOpen: () => void;
}

interface ChildNodeData extends Record<string, unknown> {
  entry: TopologyDirEntry;
  /** 中文 override（来自 root.child_labels） */
  labelOverride?: string;
  onOpen: () => void;
}

function RootNode({ data }: NodeProps<Node<RootNodeData>>) {
  const { root, expanded, onToggle, onOpen } = data;
  return (
    <div
      className={cn(
        "relative overflow-hidden rounded-xl border-2 bg-card px-4 py-3 pl-6 shadow-md transition-all",
        root.exists
          ? "border-primary-300 dark:border-primary-700"
          : "border-dashed border-warning-strong/60 opacity-70",
        "hover:border-primary-500",
      )}
      style={{ minWidth: 220 }}
    >
      {/* 左侧拖拽手柄条（node dragHandle 绑定的选择器） */}
      <div
        className="drag-handle-zone absolute left-0 top-0 h-full w-3 cursor-grab bg-gradient-to-b from-primary-400/60 to-primary-600/60 transition-colors hover:from-primary-500 hover:to-primary-700 active:cursor-grabbing"
        title="拖动我来移动节点"
      />
      {/* 隐藏 handle —— React Flow 仍需要 source/target 锚点画线 */}
      <Handle type="target" position={Position.Top} className="!opacity-0" />
      <Handle type="source" position={Position.Bottom} className="!opacity-0" />

      <div className="flex items-start justify-between gap-2">
        <button
          type="button"
          onClick={onToggle}
          className="flex min-w-0 flex-1 items-start gap-1.5 text-left"
          title={expanded ? "收起" : "展开下一层"}
        >
          {expanded ? (
            <ChevronDown className="mt-0.5 h-4 w-4 shrink-0 text-primary-600" />
          ) : (
            <ChevronRight className="mt-0.5 h-4 w-4 shrink-0 text-muted-foreground" />
          )}
          <div className="min-w-0 flex-1">
            <div className="font-serif text-sm font-semibold leading-tight">
              {root.label}
            </div>
            <div className="mt-0.5 truncate font-mono text-[10px] text-muted-foreground">
              {root.absolute_path}
            </div>
            {root.note && (
              <div className="mt-1 line-clamp-2 text-[11px] text-muted-foreground">
                {root.note}
              </div>
            )}
            {!root.exists && (
              <div className="mt-1 text-[10px] text-warning-strong">
                ⚠ 路径不存在
              </div>
            )}
          </div>
        </button>
        <button
          type="button"
          onClick={onOpen}
          disabled={!root.exists}
          className={cn(
            "shrink-0 rounded-md p-1 transition-colors",
            root.exists
              ? "text-muted-foreground hover:bg-muted hover:text-foreground"
              : "cursor-not-allowed text-muted-foreground/40",
          )}
          title="explorer 打开"
        >
          <ExternalLink className="h-3.5 w-3.5" />
        </button>
      </div>
    </div>
  );
}

function ChildNode({ data }: NodeProps<Node<ChildNodeData>>) {
  const { entry, labelOverride, onOpen } = data;
  const Icon = entry.is_dir ? Folder : FileText;
  return (
    <div
      className="relative overflow-hidden rounded-lg border border-border bg-card pl-2 pr-2.5 py-1.5 shadow-sm transition-all hover:border-primary-400"
      style={{ minWidth: 140 }}
    >
      {/* 拖拽手柄条 */}
      <div
        className="drag-handle-zone absolute left-0 top-0 h-full w-1.5 cursor-grab bg-muted-foreground/30 transition-colors hover:bg-primary-500 active:cursor-grabbing"
        title="拖动我来移动节点"
      />
      <Handle type="target" position={Position.Top} className="!opacity-0" />

      <div className="flex items-start justify-between gap-1.5 pl-1">
        <div className="flex min-w-0 flex-1 items-start gap-1">
          <Icon className="mt-0.5 h-3 w-3 shrink-0 text-muted-foreground" />
          <div className="min-w-0 flex-1">
            <div className="truncate text-[11px] font-medium leading-tight">
              {labelOverride ?? entry.name}
            </div>
            {labelOverride && (
              <div className="mt-0.5 truncate font-mono text-[9px] text-muted-foreground">
                {entry.name}
              </div>
            )}
          </div>
        </div>
        <button
          type="button"
          onClick={onOpen}
          className="shrink-0 rounded p-0.5 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
          title={entry.is_dir ? "explorer 打开" : "在默认应用打开"}
        >
          <ExternalLink className="h-3 w-3" />
        </button>
      </div>
    </div>
  );
}

// =====================================================================
// GroupNode — 虚拟分组节点（方案 A v2）
// =====================================================================

interface GroupNodeData extends Record<string, unknown> {
  group: VirtualGroup;
  expanded: boolean;
  onToggle: () => void;
}

function GroupNode({ data }: NodeProps<Node<GroupNodeData>>) {
  const { group, expanded, onToggle } = data;
  const Icon = iconForGroup(group.icon);
  return (
    <div
      className={cn(
        "relative overflow-hidden rounded-xl border bg-card/90 pl-5 pr-3 py-2 shadow-sm backdrop-blur transition-all",
        "border-primary-200/70 dark:border-primary-800/60",
        "hover:border-primary-500",
      )}
      style={{ minWidth: 180 }}
    >
      {/* 拖拽手柄条 */}
      <div
        className="drag-handle-zone absolute left-0 top-0 h-full w-2 cursor-grab bg-primary-200/80 transition-colors hover:bg-primary-500 active:cursor-grabbing dark:bg-primary-800/80"
        title="拖动我来移动节点"
      />
      <Handle type="target" position={Position.Top} className="!opacity-0" />
      <Handle type="source" position={Position.Bottom} className="!opacity-0" />

      <button
        type="button"
        onClick={onToggle}
        className="flex w-full items-start gap-2 text-left"
        title={expanded ? "收起分组" : "展开分组内容"}
      >
        <Icon className="mt-0.5 h-4 w-4 shrink-0 text-primary-600 dark:text-primary-400" />
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-1.5 text-[13px] font-medium leading-tight">
            <span className="truncate">{group.label}</span>
            <span className="shrink-0 rounded bg-muted/60 px-1 py-px font-mono text-[9px] text-muted-foreground">
              {group.paths.length}
            </span>
          </div>
          {group.note && (
            <div className="mt-0.5 line-clamp-1 text-[10px] text-muted-foreground">
              {group.note}
            </div>
          )}
        </div>
        {expanded ? (
          <ChevronDown className="mt-1 h-3.5 w-3.5 shrink-0 text-primary-600" />
        ) : (
          <ChevronRight className="mt-1 h-3.5 w-3.5 shrink-0 text-muted-foreground" />
        )}
      </button>
    </div>
  );
}

// =====================================================================
// 布局算法
// =====================================================================

interface BuildArgs {
  topology: Topology;
  expanded: Set<string>;
  childMap: Record<string, TopologyDirEntry[]>;
  refs: TopologyRefEdge[];
  onToggleExpand: (id: string) => void;
  onOpenDir: (absolutePath: string) => void;
}

// 金字塔布局 y 坐标（manual · manual children · drives · drive groups/children · group paths）
const MANUAL_Y = 0;
const MANUAL_CHILD_Y = 220;
const DRIVE_Y = 440;
const DRIVE_CHILD_Y = 660; // drive 展开第一层（groups 或 physical children）
const GROUP_CHILD_Y = 880; // group 展开后的 paths 层（v2 only）

// 水平间距
const DRIVE_GAP_X = 320;
const CHILD_GAP_X = 170;
const GROUP_GAP_X = 190; // 虚拟组间距略大（含 icon + label + count）

/** Windows / POSIX 路径取 basename */
function basename(p: string): string {
  const parts = p.split(/[\\/]+/).filter(Boolean);
  return parts[parts.length - 1] || p;
}

function buildGraph({
  topology,
  expanded,
  childMap,
  refs,
  onToggleExpand,
  onOpenDir,
}: BuildArgs): { nodes: Node[]; edges: Edge[] } {
  const nodes: Node[] = [];
  const edges: Edge[] = [];

  // 分离枢纽（manual）和盘符节点
  const manual = topology.roots.find((r) => r.id === "manual");
  const drives = topology.roots.filter((r) => r.id !== "manual");

  // ========== Layer 0: Manual 枢纽居中顶部 ==========
  if (manual) {
    const manualId = `root:${manual.id}`;
    nodes.push({
      id: manualId,
      type: "root",
      position: { x: 0, y: MANUAL_Y },
      dragHandle: ".drag-handle-zone",
      data: {
        root: manual,
        expanded: expanded.has(manual.id),
        onToggle: () => onToggleExpand(manual.id),
        onOpen: () => onOpenDir(manual.absolute_path),
      } satisfies RootNodeData,
    });

    // ========== Layer 1: Manual children（展开时）==========
    if (expanded.has(manual.id)) {
      const children = childMap[manual.id] ?? [];
      const totalW = Math.max(0, (children.length - 1) * CHILD_GAP_X);
      children.forEach((entry, ci) => {
        const childX = ci * CHILD_GAP_X - totalW / 2;
        const childId = `child:${manual.id}:${entry.name}`;
        const labelOverride = manual.child_labels[entry.name];

        nodes.push({
          id: childId,
          type: "child",
          position: { x: childX, y: MANUAL_CHILD_Y },
          dragHandle: ".drag-handle-zone",
          data: {
            entry,
            labelOverride,
            onOpen: () => onOpenDir(entry.absolute_path),
          } satisfies ChildNodeData,
        });

        edges.push({
          id: `e:expand:${manual.id}:${entry.name}`,
          source: manualId,
          target: childId,
          style: { stroke: "rgb(148 163 184 / 0.5)", strokeWidth: 1 },
          markerEnd: { type: MarkerType.Arrow, color: "rgb(148 163 184 / 0.7)" },
        });
      });
    }
  }

  // ========== Layer 2: Drives 横排 ==========
  const totalDrivesW = Math.max(0, (drives.length - 1) * DRIVE_GAP_X);
  drives.forEach((drive, di) => {
    const driveX = di * DRIVE_GAP_X - totalDrivesW / 2;
    const driveId = `root:${drive.id}`;

    nodes.push({
      id: driveId,
      type: "root",
      position: { x: driveX, y: DRIVE_Y },
      dragHandle: ".drag-handle-zone",
      data: {
        root: drive,
        expanded: expanded.has(drive.id),
        onToggle: () => onToggleExpand(drive.id),
        onOpen: () => onOpenDir(drive.absolute_path),
      } satisfies RootNodeData,
    });

    // ========== Layer 3: Drive 展开（v2 虚拟组 / v1 物理子目录 二选一）==========
    if (expanded.has(drive.id)) {
      const groups = topology.drive_groups?.[drive.id] ?? [];

      if (groups.length > 0) {
        // v2：显示虚拟分组节点
        const totalGroupsW = Math.max(0, (groups.length - 1) * GROUP_GAP_X);
        groups.forEach((group, gi) => {
          const groupX = driveX + gi * GROUP_GAP_X - totalGroupsW / 2;
          const groupNodeId = `group:${group.id}`;
          const groupKey = `group:${group.id}`;
          const groupExpanded = expanded.has(groupKey);

          nodes.push({
            id: groupNodeId,
            type: "group",
            position: { x: groupX, y: DRIVE_CHILD_Y },
            dragHandle: ".drag-handle-zone",
            data: {
              group,
              expanded: groupExpanded,
              onToggle: () => onToggleExpand(groupKey),
            } satisfies GroupNodeData,
          });

          edges.push({
            id: `e:drive-group:${drive.id}:${group.id}`,
            source: driveId,
            target: groupNodeId,
            style: { stroke: "rgb(99 102 241 / 0.45)", strokeWidth: 1.2 },
            markerEnd: { type: MarkerType.Arrow, color: "rgb(99 102 241 / 0.7)" },
          });

          // Layer 4: group 展开后的 paths
          if (groupExpanded && group.paths.length > 0) {
            const totalPathsW = Math.max(0, (group.paths.length - 1) * CHILD_GAP_X);
            group.paths.forEach((p, pi) => {
              const pathX = groupX + pi * CHILD_GAP_X - totalPathsW / 2;
              const pathId = `child:${group.id}:${p}`;
              const name = basename(p);
              nodes.push({
                id: pathId,
                type: "child",
                position: { x: pathX, y: GROUP_CHILD_Y },
                dragHandle: ".drag-handle-zone",
                data: {
                  entry: {
                    name,
                    absolute_path: p,
                    is_dir: true,
                  },
                  labelOverride: undefined,
                  onOpen: () => onOpenDir(p),
                } satisfies ChildNodeData,
              });

              edges.push({
                id: `e:group-path:${group.id}:${p}`,
                source: groupNodeId,
                target: pathId,
                style: { stroke: "rgb(148 163 184 / 0.5)", strokeWidth: 1 },
                markerEnd: { type: MarkerType.Arrow, color: "rgb(148 163 184 / 0.7)" },
              });
            });
          }
        });
      } else {
        // v1 模式：listDir 物理子目录（原行为 · 向后兼容）
        const children = childMap[drive.id] ?? [];
        const totalChildW = Math.max(0, (children.length - 1) * CHILD_GAP_X);
        children.forEach((entry, ci) => {
          const childX = driveX + ci * CHILD_GAP_X - totalChildW / 2;
          const childId = `child:${drive.id}:${entry.name}`;
          const labelOverride = drive.child_labels[entry.name];

          nodes.push({
            id: childId,
            type: "child",
            position: { x: childX, y: DRIVE_CHILD_Y },
            dragHandle: ".drag-handle-zone",
            data: {
              entry,
              labelOverride,
              onOpen: () => onOpenDir(entry.absolute_path),
            } satisfies ChildNodeData,
          });

          edges.push({
            id: `e:expand:${drive.id}:${entry.name}`,
            source: driveId,
            target: childId,
            style: { stroke: "rgb(148 163 184 / 0.5)", strokeWidth: 1 },
            markerEnd: { type: MarkerType.Arrow, color: "rgb(148 163 184 / 0.7)" },
          });
        });
      }
    }
  });

  // 用户配置的 topology.edges（root.id → root.id）
  const allRoots = topology.roots;
  const rootIdSet = new Set(allRoots.map((r) => r.id));
  for (const e of topology.edges) {
    if (!rootIdSet.has(e.source) || !rootIdSet.has(e.target)) continue;
    edges.push({
      id: `e:cfg:${e.source}:${e.target}`,
      source: `root:${e.source}`,
      target: `root:${e.target}`,
      label: e.label ?? undefined,
      labelStyle: { fontSize: 10, fill: "rgb(100 116 139)" },
      labelBgPadding: [4, 2],
      labelBgStyle: { fill: "rgb(248 250 252)", fillOpacity: 0.9 },
      style: { stroke: "rgb(99 102 241 / 0.7)", strokeWidth: 1.5 },
      markerEnd: {
        type: MarkerType.ArrowClosed,
        color: "rgb(99 102 241)",
      },
    });
  }

  // references 边（来自 frontmatter）
  for (const ref of refs) {
    if (!ref.target_id) continue;
    if (!rootIdSet.has(ref.target_id)) continue;
    // 找到 source 所属的 root
    const sourceRoot = allRoots.find((r) =>
      ref.source_path
        .toLowerCase()
        .startsWith(r.absolute_path.toLowerCase()),
    );
    if (!sourceRoot || sourceRoot.id === ref.target_id) continue;
    edges.push({
      id: `e:ref:${sourceRoot.id}:${ref.target_id}:${ref.source_path}`,
      source: `root:${sourceRoot.id}`,
      target: `root:${ref.target_id}`,
      label: ref.label ?? undefined,
      labelStyle: { fontSize: 10, fill: "rgb(100 116 139)" },
      labelBgPadding: [4, 2],
      labelBgStyle: { fill: "rgb(248 250 252)", fillOpacity: 0.9 },
      style: {
        stroke: "rgb(34 197 94 / 0.6)",
        strokeWidth: 1.2,
        strokeDasharray: "4 3",
      },
      markerEnd: {
        type: MarkerType.ArrowClosed,
        color: "rgb(34 197 94)",
      },
    });
  }

  return { nodes, edges };
}
