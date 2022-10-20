use std::fs::File;
use std::io::Read;
use std::sync::Arc;

use anyhow::{bail, Context};
use tempfile::tempdir;
use tokio::test;
use wasi_common::pipe::{ReadPipe, WritePipe};
use wasi_common::WasiDir;
use wasmtime::{Engine, Linker, Module, Store, Trap};
use wasmtime_vfs_ledger::Ledger;
use wasmtime_wasi::WasiCtxBuilder;

fn wash<I: Read + Sync + Send + 'static>(
    dir: Box<dyn WasiDir>,
    stdin: impl Into<ReadPipe<I>>,
) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
    let engine = Engine::default();
    let module = Module::from_binary(&engine, include_bytes!(env!("CARGO_BIN_FILE_WASH")))
        .context("failed to compile `wash`")?;
    let mut linker = Linker::new(&engine);
    wasmtime_wasi::add_to_linker(&mut linker, |s| s).context("failed to link WASI")?;

    let stdout = WritePipe::new_in_memory();
    let stderr = WritePipe::new_in_memory();
    let code = {
        let mut ctx = WasiCtxBuilder::new()
            .arg("main.wasm")
            .context("failed to set argv[0]")?
            .stdin(Box::new(stdin.into()))
            .stdout(Box::new(stdout.clone()))
            .stderr(Box::new(stderr.clone()))
            .build();
        ctx.push_preopened_dir(dir, "/")
            .context("failed to push directory")?;

        let mut store = Store::new(&engine, ctx);
        linker
            .module(&mut store, "", &module)
            .context("failed to link Wasm module")?;
        linker
            .get_default(&mut store, "")
            .context("failed to get default function")?
            .typed::<(), (), _>(&store)
            .context("failed to assert default function type")?
            .call(&mut store, ())
            .as_ref()
            .map_err(Trap::i32_exit_status)
            .expect_err("`wash` did not set an exit code, did you forget to execute `exit`?")
            .context("failed to execute default function")?
    };
    let stdout = stdout
        .try_into_inner()
        .expect("failed to acquire stdout")
        .into_inner();
    let stderr = stderr
        .try_into_inner()
        .expect("failed to acquire stderr")
        .into_inner();
    if code != 0 {
        bail!(
            r#"`wash` exited with code `{code}`
stdout: {}
stderr: {}
"#,
            String::from_utf8_lossy(&stdout),
            String::from_utf8_lossy(&stderr),
        )
    }
    Ok((stdout, stderr))
}

#[test]
async fn wasi_sync_dir() -> anyhow::Result<()> {
    let tmp = tempdir().context("failed to create a temporary directory")?;
    let dir = File::open(&tmp)
        .map(wasmtime_wasi::sync::Dir::from_std_file)
        .map(wasmtime_wasi::sync::dir::Dir::from_cap_std)
        .map(Box::new)
        .with_context(|| format!("failed to open `{}`", tmp.path().display()))?;
    let (out, err) = wash(
        dir,
        r#"ls /
echo 'test' > test
cat test
exit
"#,
    )
    .context("failed to execute `wash`")?;

    let out = String::from_utf8_lossy(&out);
    let err = String::from_utf8_lossy(&err);
    assert_eq!(
        out,
        r#"
test
"#,
        "{err}"
    );
    Ok(())
}

#[test]
#[ignore = "tmpfs is currently broken on WASI"] // TODO: Make tmpfs run on WASI
async fn tmpfs() -> anyhow::Result<()> {
    let ledger = Ledger::new();
    let dir = wasmtime_vfs_tmpfs::Builder::from(Arc::clone(&ledger))
        .add("file", Some(b"file".to_vec()))
        .await
        .context("failed to add `/file` to tmpfs")?
        .add("dir", None)
        .await
        .context("failed to add `/dir/` to tmpfs")?
        .add("dir/file", Some(b"file".to_vec()))
        .await
        .context("failed to add `/dir/file` to tmpfs")?
        .add("dir/dir", None)
        .await
        .context("failed to add `/dir/dir/` to tmpfs")?
        .build();
    let (out, err) = wash(
        dir,
        r#"ls /
echo 'test' > test
cat test
exit
"#,
    )
    .context("failed to execute `wash`")?;

    let out = String::from_utf8_lossy(&out);
    let err = String::from_utf8_lossy(&err);
    assert_eq!(
        out,
        r#"
test
"#,
        "{err}"
    );
    Ok(())
}
