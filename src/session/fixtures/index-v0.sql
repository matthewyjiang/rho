pragma user_version = 0;
create table sessions (
    workspace_key text not null,
    cwd text not null,
    id text not null,
    path text not null,
    created_at integer not null,
    updated_at integer not null,
    message_count integer not null default 0,
    last_user_message text,
    file_size integer,
    file_mtime integer,
    primary key (workspace_key, id)
);
insert into sessions (
    workspace_key, cwd, id, path, created_at, updated_at, message_count,
    last_user_message, file_size, file_mtime
) values (
    'fixture-workspace', '/fixture', 'fixture-session', '/fixture/session.jsonl',
    10, 20, 2, 'historical prompt', 100, 20
);
