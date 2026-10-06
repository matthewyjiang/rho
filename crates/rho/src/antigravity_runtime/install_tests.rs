use pretty_assertions::assert_eq;
use tokio::io::AsyncReadExt as _;

use super::*;
use crate::antigravity_runtime::executable::{harness_location, HarnessLocation};

/// Zip bytes holding `entries` (name, contents), stored uncompressed.
fn zip_bytes(entries: &[(&str, &str)]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, contents) in entries {
        zip.start_file(*name, options).unwrap();
        zip.write_all(contents.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn server_zip() -> Vec<u8> {
    zip_bytes(&[(ANTIGRAVITY_PROGRAM, "server"), (HARNESS_FILE, "harness")])
}

/// Serve `body` once over loopback HTTP; returns its URL.
async fn serve_once(body: Vec<u8>) -> &'static str {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/archive.zip", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buf = [0; 1024];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let read = stream.read(&mut buf).await.unwrap();
            if read == 0 {
                return;
            }
            request.extend_from_slice(&buf[..read]);
        }
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        // The client hangs up early on an oversized body; that is expected.
        let _ = stream.write_all(head.as_bytes()).await;
        let _ = stream.write_all(&body).await;
    });
    Box::leak(url.into_boxed_str())
}

/// Pin metadata that `body` satisfies.
fn pin_for(url: &'static str, body: &[u8]) -> PinnedArchive {
    PinnedArchive {
        url,
        sha256: Box::leak(hex::encode(Sha256::digest(body)).into_boxed_str()),
        archive_bytes: body.len() as u64,
        unpacked_bytes: 0,
    }
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// A managed root with an older and a newer release, interrupted-install
/// debris, and a directory Rho does not recognize.
fn populated_root() -> (tempfile::TempDir, ManagedRoot) {
    let home = tempfile::tempdir().unwrap();
    let root = ManagedRoot::new(home.path()).unwrap();
    for dir in ["1.2.0", "1.10.0", ".staging-old", "notes"] {
        std::fs::create_dir_all(root.0.join(dir)).unwrap();
    }
    (home, root)
}

// Covers: a verified download lands where the runtime looks (server with its
// harness beside it, executable) and prunes only older releases and debris:
// a newer Rho's release and unknown entries survive.
// Owner: managed install, download through publish.
#[tokio::test]
async fn install_publishes_a_verified_archive_and_prunes_only_older_releases() {
    let (_home, root) = populated_root();
    let body = server_zip();
    let archive = pin_for(serve_once(body.clone()).await, &body);

    let server = install(&archive, &root).await.unwrap();

    assert_eq!(server, root.server());
    assert_eq!(
        names(&root.0),
        [".install.lock", "1.10.0", PINNED_VERSION, "notes"]
    );
    assert_eq!(
        harness_location(&server, None),
        HarnessLocation::Beside(
            std::fs::canonicalize(root.release_dir())
                .unwrap()
                .join(HARNESS_FILE)
        )
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&server).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
    }
}

// Covers: a download that does not match the pin (checksum, longer or
// shorter body) or an archive with another layout publishes nothing and
// prunes nothing.
// Owner: managed install verification boundary.
#[tokio::test]
async fn install_rejects_unverified_downloads_without_touching_existing_releases() {
    let good = server_zip();
    type Tamper = fn(PinnedArchive) -> PinnedArchive;
    let cases: [(&str, Vec<u8>, Tamper); 5] = [
        ("checksum", good.clone(), |pin| PinnedArchive {
            sha256: "0000000000000000000000000000000000000000000000000000000000000000",
            ..pin
        }),
        ("oversized", good.clone(), |pin| PinnedArchive {
            archive_bytes: pin.archive_bytes - 1,
            ..pin
        }),
        ("truncated", good.clone(), |pin| PinnedArchive {
            archive_bytes: pin.archive_bytes + 1,
            ..pin
        }),
        (
            "missing harness",
            zip_bytes(&[(ANTIGRAVITY_PROGRAM, "server")]),
            |pin| pin,
        ),
        (
            "nested harness",
            zip_bytes(&[
                (ANTIGRAVITY_PROGRAM, "server"),
                ("bin/localharness_external", "harness"),
            ]),
            |pin| pin,
        ),
    ];
    for (case, body, tamper) in cases {
        let (_home, root) = populated_root();
        let before = names(&root.0);
        let archive = tamper(pin_for(serve_once(body.clone()).await, &body));

        assert!(install(&archive, &root).await.is_err(), "{case}");

        let mut expected = before;
        expected.push(LOCK_FILE.to_owned());
        expected.sort();
        assert_eq!(names(&root.0), expected, "{case}");
        assert_eq!(root.installed_server(), None, "{case}");
    }
}

// Covers: a relative `RHO_HOME` still yields an absolute server path, so a
// workflow workspace or child working directory cannot reinterpret it.
// Owner: managed install layout.
#[test]
fn managed_root_anchors_a_relative_home() {
    let root = ManagedRoot::new(Path::new("relative-rho-home")).unwrap();
    assert!(root.server().is_absolute(), "{:?}", root.server());
}

// Covers: the download needs an explicit yes; anything else declines.
// Owner: install consent prompt.
#[test]
fn confirm_requires_an_explicit_yes() {
    let archive = archive_for("linux", "x86_64").unwrap();
    let cases = [
        ("y\n", true),
        (" YES \n", true),
        ("\n", false),
        ("n\n", false),
        ("sure\n", false),
        ("", false),
    ];
    for (answer, expected) in cases {
        let agreed = confirm(
            &archive,
            Path::new("/rho/runtimes/antigravity-acp/1.3.0"),
            &mut answer.as_bytes(),
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(agreed, expected, "{answer:?}");
    }
}
