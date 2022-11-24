mod util;

use std::sync::Arc;

use anyhow::Context;
use tempfile::tempdir;
use tokio::test;
use wasmtime_vfs_dir::Directory;
use wasmtime_vfs_file::File;
use wasmtime_vfs_ledger::Ledger;
use wasmtime_vfs_memory::Node;

#[test]
#[cfg_attr(feature = "interactive", serial_test::serial)]
async fn wasi_sync_dir() -> anyhow::Result<()> {
    const CMD: &str = r#"ls /
echo 'test' > test
cat test
"#;

    const OUT: &str = r#"
test
"#;

    // Set up a temporary directory.
    let tmp = tempdir().context("failed to create a temporary directory")?;
    let dir = std::fs::File::open(&tmp)
        .map(wasmtime_wasi::sync::Dir::from_std_file)
        .map(wasmtime_wasi::sync::dir::Dir::from_cap_std)
        .map(Box::new)
        .with_context(|| format!("failed to open `{}`", tmp.path().display()))?;

    // Run the script and test the output.
    let (out, err) = util::wash(dir, CMD)
        .await
        .context("failed to execute `wash`")?;
    if cfg!(not(feature = "interactive")) {
        let out = String::from_utf8_lossy(&out);
        let err = String::from_utf8_lossy(&err);
        assert_eq!(out, OUT, "{err}");
    }
    Ok(())
}

#[test]
#[cfg_attr(feature = "interactive", serial_test::serial)]
async fn tmpfs() -> anyhow::Result<()> {
    const CMD: &str = r#"ls /
echo 'test' > test
ls /
cat test
ls /dir
"#;

    const OUT: &str = r#"dir file
dir file test
test
a.file b.dir
"#;

    let tree = [
        ("/file", Some(b"file")),
        ("/dir", None),
        ("/dir/a.file", Some(b"file")),
        ("/dir/b.dir", None),
    ];

    // Construct the tmpfs tree.
    let root = Directory::root(Ledger::new(), Some(Arc::new(File::new)));
    for (path, data) in tree {
        let parent = root.get(path.rsplit_once('/').unwrap().0).await.unwrap();
        let child: Arc<dyn Node> = match data {
            Some(data) => File::with_data(parent, *data),
            None => Directory::new(parent, Some(Arc::new(File::new))),
        };

        root.attach(path, child).await.unwrap();
    }
    let root = root.open_dir().await.unwrap();

    // Run the script and test the output.
    let (out, err) = util::wash(root, CMD)
        .await
        .context("failed to execute `wash`")?;
    if cfg!(not(feature = "interactive")) {
        let out = String::from_utf8_lossy(&out);
        let err = String::from_utf8_lossy(&err);
        assert_eq!(out, OUT, "{err}");
    }
    Ok(())
}
