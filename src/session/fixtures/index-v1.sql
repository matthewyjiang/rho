pragma user_version = 1;
create table sessions (
    workspace_key text not null,
    cwd text not null,
    id text not null,
    path text not null,
    created_at integer not null,
    updated_at integer not null,
    message_count integer not null default 0,
    title text,
    first_user_message text,
    last_user_message text,
    file_size integer,
    file_mtime integer,
    primary key (workspace_key, id)
);
create index sessions_workspace_updated_idx
    on sessions(workspace_key, updated_at desc);
create index sessions_workspace_id_idx
    on sessions(workspace_key, id);
insert into sessions values (
    'fixture-workspace', '/fixture', 'fixture-session', '/fixture/session.jsonl',
    10, 20, 2, 'fixture title', 'first prompt', 'last prompt', 100, 20
);
