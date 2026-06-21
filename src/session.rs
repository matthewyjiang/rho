use std::{
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::model::Message;

const SESSION_VERSION: u32 = 1;

#[derive(Clone, Debug)]
pub struct Session {
    id: String,
    path: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SessionEntry {
    Session {
        version: u32,
        id: String,
        timestamp: String,
        cwd: PathBuf,
    },
    Message {
        timestamp: String,
        message: Message,
    },
}

impl Session {
    pub fn open_by_id(cwd: &Path, id_prefix: &str) -> anyhow::Result<(Self, Vec<Message>)> {
        Self::open_by_id_in_root(&session_root()?, cwd, id_prefix)
    }

    fn open_by_id_in_root(
        session_root: &Path,
        cwd: &Path,
        id_prefix: &str,
    ) -> anyhow::Result<(Self, Vec<Message>)> {
        let dir = session_dir_in_root(session_root, cwd);
        fs::create_dir_all(&dir)?;
        let matches = matching_session_files(&dir, id_prefix)?;
        match matches.as_slice() {
            [] => anyhow::bail!("no session found matching '{id_prefix}'"),
            [path] => {
                let id = session_id_from_path(path).ok_or_else(|| {
                    anyhow::anyhow!("session file has invalid name: {}", path.display())
                })?;
                let messages = read_messages(path)?;
                Ok((
                    Self {
                        id,
                        path: path.clone(),
                    },
                    messages,
                ))
            }
            _ => anyhow::bail!("multiple sessions match '{id_prefix}'; use a longer UUID prefix"),
        }
    }

    pub fn create(cwd: &Path) -> anyhow::Result<Self> {
        Self::create_in_root(&session_root()?, cwd)
    }

    fn create_in_root(session_root: &Path, cwd: &Path) -> anyhow::Result<Self> {
        let dir = session_dir_in_root(session_root, cwd);
        fs::create_dir_all(&dir)?;
        let id = Uuid::new_v4().to_string();
        let path = dir.join(format!("{}_{}.jsonl", timestamp_for_filename(), id));
        let session = Self {
            id: id.clone(),
            path,
        };
        session.append_entry(&SessionEntry::Session {
            version: SESSION_VERSION,
            id,
            timestamp: timestamp(),
            cwd: cwd.to_path_buf(),
        })?;
        Ok(session)
    }

    pub fn append_message(&self, message: &Message) -> anyhow::Result<()> {
        self.append_entry(&SessionEntry::Message {
            timestamp: timestamp(),
            message: message.clone(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    fn append_entry(&self, entry: &SessionEntry) -> anyhow::Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        serde_json::to_writer(&mut file, entry)?;
        file.write_all(b"\n")?;
        file.sync_data()?;
        Ok(())
    }
}

fn read_messages(path: &Path) -> anyhow::Result<Vec<Message>> {
    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut messages = Vec::new();
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<SessionEntry>(&line)? {
            SessionEntry::Session { .. } => {}
            SessionEntry::Message { message, .. } => messages.push(message),
        }
    }
    Ok(messages)
}

fn matching_session_files(dir: &Path, id_prefix: &str) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let id = session_id_from_path(&path)?;
            id.starts_with(id_prefix).then_some(path)
        })
        .collect::<Vec<_>>();
    files.sort();
    Ok(files)
}

fn session_id_from_path(path: &Path) -> Option<String> {
    if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
        return None;
    }
    path.file_stem()?
        .to_str()?
        .rsplit_once('_')
        .map(|(_, id)| id.to_string())
}

fn session_root() -> anyhow::Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
    Ok(home.join(".rho").join("sessions"))
}

fn session_dir_in_root(session_root: &Path, cwd: &Path) -> PathBuf {
    session_root.join(workspace_key(cwd))
}

fn workspace_key(cwd: &Path) -> String {
    format!("{}-{:016x}", encode_cwd(cwd), stable_path_hash(cwd))
}

fn encode_cwd(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect()
}

fn stable_path_hash(path: &Path) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    path.to_string_lossy()
        .as_bytes()
        .iter()
        .fold(FNV_OFFSET, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME)
        })
}

fn timestamp() -> String {
    unix_timestamp_secs().to_string()
}

fn timestamp_for_filename() -> String {
    unix_timestamp_secs().to_string()
}

fn unix_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_and_loads_messages() {
        let root = temp_session_root();
        let cwd = temp_cwd();
        let session = Session::create_in_root(&root, &cwd).unwrap();
        session
            .append_message(&Message::user_text("hello"))
            .unwrap();
        session
            .append_message(&Message::assistant_text("hi"))
            .unwrap();

        let (_session, messages) = Session::open_by_id_in_root(&root, &cwd, session.id()).unwrap();
        assert_eq!(messages.len(), 2);
        assert!(matches!(&messages[0], Message::User(_)));
        assert!(matches!(&messages[1], Message::Assistant(_)));
    }

    #[test]
    fn opens_session_by_uuid_prefix() {
        let root = temp_session_root();
        let cwd = temp_cwd();
        let session = Session::create_in_root(&root, &cwd).unwrap();
        session
            .append_message(&Message::user_text("prefix match"))
            .unwrap();

        let prefix = &session.id()[..8];
        let (opened, messages) = Session::open_by_id_in_root(&root, &cwd, prefix).unwrap();

        assert_eq!(opened.id(), session.id());
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn errors_when_uuid_prefix_is_ambiguous() {
        let root = temp_session_root();
        let cwd = temp_cwd();
        write_minimal_session_file(&root, &cwd, "aaaaaaaa-1111-4111-8111-111111111111");
        write_minimal_session_file(&root, &cwd, "aaaaaaaa-2222-4222-8222-222222222222");

        let err = Session::open_by_id_in_root(&root, &cwd, "aaaaaaaa").unwrap_err();

        assert!(err.to_string().contains("multiple sessions match"));
    }

    #[test]
    fn errors_when_uuid_prefix_is_missing() {
        let root = temp_session_root();
        let cwd = temp_cwd();

        let err = Session::open_by_id_in_root(&root, &cwd, "missing").unwrap_err();

        assert!(err.to_string().contains("no session found"));
    }

    #[test]
    fn stores_sessions_under_session_root_workspace_key() {
        let root = temp_session_root();
        let cwd = temp_cwd();
        let session = Session::create_in_root(&root, &cwd).unwrap();
        let expected_parent = root.join(workspace_key(&cwd));

        assert_eq!(session.path().parent(), Some(expected_parent.as_path()));
    }

    #[test]
    fn workspace_key_avoids_separator_collisions() {
        let slash_path = PathBuf::from("/tmp/rho-workspace/a/b");
        let dash_path = PathBuf::from("/tmp/rho-workspace/a-b");

        assert_eq!(encode_cwd(&slash_path), encode_cwd(&dash_path));
        assert_ne!(workspace_key(&slash_path), workspace_key(&dash_path));
    }

    fn temp_session_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!("rho-session-root-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn temp_cwd() -> PathBuf {
        let cwd = std::env::temp_dir().join(format!("rho-session-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&cwd).unwrap();
        cwd
    }

    fn write_minimal_session_file(root: &Path, cwd: &Path, id: &str) {
        let dir = session_dir_in_root(root, cwd);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("test_{id}.jsonl"));
        fs::write(
            path,
            serde_json::json!({
                "type": "session",
                "version": SESSION_VERSION,
                "id": id,
                "timestamp": "0",
                "cwd": cwd,
            })
            .to_string()
                + "\n",
        )
        .unwrap();
    }
}
