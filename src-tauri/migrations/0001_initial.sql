-- HomeBase SQLite schema
-- 来源：v3 §7 (v2 主体) + §21 (v3 增量 Tasks/Skills/Chat) + §23 (Plugin → Extension 改名)
-- 数据库位置：~/AppData/HomeBase/homebase.db
-- 创建：Phase 13.1 Day 1-2 (2026-05-03)

------------------------------------------------------------------------------
-- v2 主体（v3 §7）
------------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY,
    value TEXT,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- Recent Activity 数据源
CREATE TABLE IF NOT EXISTS events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    event_type TEXT NOT NULL,         -- 'agent.started' | 'secret.tested' | ...
    actor TEXT,
    target TEXT,
    details TEXT,                      -- JSON
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_events_created ON events(created_at DESC);

CREATE TABLE IF NOT EXISTS notifications (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    severity TEXT NOT NULL,            -- 'info' | 'warning' | 'error'
    title TEXT NOT NULL,
    body TEXT,
    read INTEGER DEFAULT 0,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS agent_sessions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_name TEXT NOT NULL,
    pid INTEGER,
    started_at TIMESTAMP NOT NULL,
    ended_at TIMESTAMP,
    exit_code INTEGER,
    log_path TEXT
);
CREATE INDEX IF NOT EXISTS idx_agent_sessions_started ON agent_sessions(started_at DESC);

CREATE TABLE IF NOT EXISTS secret_health (
    short_name TEXT NOT NULL,
    checked_at TIMESTAMP NOT NULL,
    status TEXT NOT NULL,              -- 'valid' | 'invalid' | 'expired' | 'unknown'
    response_ms INTEGER,
    error TEXT,
    PRIMARY KEY (short_name, checked_at)
);

-- v3 §23 改名：plugins → extensions（避免与业界 Plugin 概念冲突）
CREATE TABLE IF NOT EXISTS extensions (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    version TEXT,
    kind TEXT,                         -- 'module' | 'action' | 'integration'
    enabled INTEGER DEFAULT 1,
    manifest_path TEXT,
    installed_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS action_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    action_id TEXT NOT NULL,
    invoked_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    duration_ms INTEGER,
    success INTEGER
);

------------------------------------------------------------------------------
-- v3 §21.1 Tasks 模块
------------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS tasks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    title TEXT NOT NULL,
    description TEXT,
    status TEXT NOT NULL DEFAULT 'pending',     -- pending | in_progress | done | cancelled
    priority TEXT DEFAULT 'medium',              -- high | medium | low
    project_id INTEGER,
    knowledge_doc_id TEXT,
    parent_task_id INTEGER,
    estimated_minutes INTEGER,
    actual_minutes INTEGER,
    due_date DATE,
    tags TEXT,                                   -- JSON array
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    completed_at TIMESTAMP,
    FOREIGN KEY (parent_task_id) REFERENCES tasks(id) ON DELETE SET NULL
);
CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks(status);
CREATE INDEX IF NOT EXISTS idx_tasks_project ON tasks(project_id);
CREATE INDEX IF NOT EXISTS idx_tasks_doc ON tasks(knowledge_doc_id);

------------------------------------------------------------------------------
-- v3 §21.2 Skills 模块（缓存表 — 源数据在文件系统）
------------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS skills (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    description TEXT,
    file_path TEXT UNIQUE NOT NULL,
    source TEXT,                       -- claude | cursor | cline | aider | general | custom
    target_agents TEXT,                -- JSON array
    tags TEXT,                          -- JSON array
    last_modified TIMESTAMP,
    indexed_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_skills_source ON skills(source);
CREATE INDEX IF NOT EXISTS idx_skills_name ON skills(name);

------------------------------------------------------------------------------
-- v3 §21.3 Chat Hub（Agents 子组件，不是顶级模块）
------------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS chat_sessions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id TEXT NOT NULL,            -- claude-code | codex | gemini | hermes | openclaw
    external_session_id TEXT,
    title TEXT,
    started_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    last_message_at TIMESTAMP,
    is_archived INTEGER DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_chat_sessions_agent ON chat_sessions(agent_id, started_at DESC);

CREATE TABLE IF NOT EXISTS chat_messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id INTEGER NOT NULL,
    parent_message_id INTEGER,         -- Tree forking
    role TEXT NOT NULL,                -- user | assistant | system | tool
    content TEXT NOT NULL,
    tokens_in INTEGER,
    tokens_out INTEGER,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    metadata TEXT,                     -- JSON: tool_calls / attachments / context_injects
    FOREIGN KEY (session_id) REFERENCES chat_sessions(id) ON DELETE CASCADE,
    FOREIGN KEY (parent_message_id) REFERENCES chat_messages(id) ON DELETE SET NULL
);
CREATE INDEX IF NOT EXISTS idx_messages_session ON chat_messages(session_id, created_at);
CREATE INDEX IF NOT EXISTS idx_messages_parent ON chat_messages(parent_message_id);
