mod dir;
mod file;

pub use dir::Directory;
pub use file::File;

#[cfg(test)]
mod test {
    use std::io::IoSliceMut;

    use wasi_common::file::{FdFlags, FileType, OFlags};
    use wasi_common::Error;
    use wasmtime_vfs_ledger::Ledger;
    use wasmtime_vfs_memory::Node;

    use super::{Directory, File};

    #[tokio::test]
    async fn test() -> Result<(), Error> {
        const FILES: &[(&str, Option<&[u8]>)] = &[
            ("foo", None),
            ("foo/bar", Some(b"abc")),
            ("foo/baz", Some(b"abc")),
            ("foo/bat", None),
            ("foo/bat/qux", Some(b"abc")),
            ("ack", None),
            ("ack/act", Some(b"abc")),
            ("zip", Some(b"abc")),
        ];

        let dir = Directory::root(Ledger::new());
        for (path, data) in FILES {
            dir.attach(path, |p| match data {
                Some(data) => Ok(File::with_data(p, *data)),
                None => Ok(Directory::new(p)),
            })
            .await?
        }
        let treefs = dir.open_dir().await?;

        let top: Vec<Result<_, _>> = treefs.readdir(0.into()).await.unwrap().collect();

        assert_eq!(top[0].as_ref().unwrap().name, ".");
        assert_eq!(top[0].as_ref().unwrap().filetype, FileType::Directory);
        assert_eq!(top[0].as_ref().unwrap().inode, 0);

        assert_eq!(top[1].as_ref().unwrap().name, "..");
        assert_eq!(top[1].as_ref().unwrap().filetype, FileType::Directory);
        assert_eq!(top[1].as_ref().unwrap().inode, 0);

        assert_eq!(top[2].as_ref().unwrap().name, "ack");
        assert_eq!(top[2].as_ref().unwrap().filetype, FileType::Directory);
        assert_eq!(top[2].as_ref().unwrap().inode, 6);

        assert_eq!(top[3].as_ref().unwrap().name, "foo");
        assert_eq!(top[3].as_ref().unwrap().filetype, FileType::Directory);
        assert_eq!(top[3].as_ref().unwrap().inode, 1);

        assert_eq!(top[4].as_ref().unwrap().name, "zip");
        assert_eq!(top[4].as_ref().unwrap().filetype, FileType::RegularFile);
        assert_eq!(top[4].as_ref().unwrap().inode, 8);

        let foo = treefs.open_dir(false, "foo").await.unwrap();
        let foo: Vec<Result<_, _>> = foo.readdir(0.into()).await.unwrap().collect();

        assert_eq!(foo[0].as_ref().unwrap().name, ".");
        assert_eq!(foo[0].as_ref().unwrap().filetype, FileType::Directory);
        assert_eq!(foo[0].as_ref().unwrap().inode, 1);

        assert_eq!(foo[1].as_ref().unwrap().name, "..");
        assert_eq!(foo[1].as_ref().unwrap().filetype, FileType::Directory);
        assert_eq!(foo[1].as_ref().unwrap().inode, 0);

        assert_eq!(foo[2].as_ref().unwrap().name, "bar");
        assert_eq!(foo[2].as_ref().unwrap().filetype, FileType::RegularFile);
        assert_eq!(foo[2].as_ref().unwrap().inode, 2);

        assert_eq!(foo[3].as_ref().unwrap().name, "bat");
        assert_eq!(foo[3].as_ref().unwrap().filetype, FileType::Directory);
        assert_eq!(foo[3].as_ref().unwrap().inode, 4);

        assert_eq!(foo[4].as_ref().unwrap().name, "baz");
        assert_eq!(foo[4].as_ref().unwrap().filetype, FileType::RegularFile);
        assert_eq!(foo[4].as_ref().unwrap().inode, 3);

        let mut qux = treefs
            .open_file(
                false,
                "foo/bat/qux",
                OFlags::empty(),
                true,
                false,
                FdFlags::empty(),
            )
            .await
            .unwrap();

        let mut buf = [0u8; 3];
        let mut bufs = [IoSliceMut::new(&mut buf)];
        let len = qux.read_vectored(&mut bufs).await.unwrap();
        assert_eq!(len, 3);
        assert_eq!(&buf, b"abc");

        Ok(())
    }
}
