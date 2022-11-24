use std::sync::Arc;

use generate::Generate;
use trust::Trust;

use wasi_common::Error;
use wasmtime_vfs_dir::Directory;
use wasmtime_vfs_memory::Node;

mod generate;
mod share;
mod sign;
mod trust;
mod verify;

pub const RS256: &[u8] = b"\x00\x00\x00\x00";
pub const RS384: &[u8] = b"\x00\x00\x00\x01";
pub const RS512: &[u8] = b"\x00\x00\x00\x02";
pub const PS256: &[u8] = b"\x00\x00\x00\x03";
pub const PS384: &[u8] = b"\x00\x00\x00\x04";
pub const PS512: &[u8] = b"\x00\x00\x00\x05";
pub const ES256K: &[u8] = b"\x00\x00\x00\x06";
pub const ES256: &[u8] = b"\x00\x00\x00\x07";
pub const ES384: &[u8] = b"\x00\x00\x00\x08";
pub const ES512: &[u8] = b"\x00\x00\x00\x09";

pub async fn new(parent: Arc<dyn Node>) -> Result<Arc<dyn Node>, Error> {
    let dir = Directory::device(parent, None);
    dir.attach("generate", Generate::new(dir.clone())).await?;
    dir.attach("trust", Trust::new(dir.clone())).await?;
    Ok(dir)
}

#[cfg(test)]
mod test {
    use std::io::{IoSlice, IoSliceMut};

    use signature::{Signature, Signer, Verifier};
    use uuid::Uuid;
    use wasi_common::file::{FdFlags, OFlags};
    use wasi_common::{Error, ErrorKind, WasiDir, WasiFile};
    use wasmtime_vfs_ledger::Ledger;

    use super::*;

    async fn root(ledger: Arc<Ledger>) -> Result<Arc<dyn Node>, Error> {
        let dir = Directory::root(ledger, None);
        dir.attach("generate", Generate::new(dir.clone())).await?;
        dir.attach("trust", Trust::new(dir.clone())).await?;
        Ok(dir)
    }

    async fn open_file(
        dir: &dyn WasiDir,
        path: &str,
        read: bool,
        write: bool,
    ) -> Box<dyn WasiFile> {
        dir.open_file(false, path, OFlags::empty(), read, write, FdFlags::empty())
            .await
            .unwrap()
    }

    async fn write(file: &mut dyn WasiFile, bytes: &[&[u8]], last: bool) -> Result<(), Error> {
        let slice = bytes.iter().map(|x| IoSlice::new(x)).collect::<Vec<_>>();

        let written = match last {
            false => file.write_vectored(&slice).await?,
            true => file.write_vectored_at(&slice, u64::MAX).await?,
        };

        assert_eq!(written, bytes.iter().map(|x| x.len()).sum::<usize>() as u64);
        Ok(())
    }

    async fn read<const N: usize>(file: &mut dyn WasiFile, last: bool) -> [u8; N] {
        let mut array = [0u8; N];
        let mut slice = [IoSliceMut::new(&mut array)];
        let read = match last {
            false => file.read_vectored(&mut slice).await.unwrap(),
            true => file.read_vectored_at(&mut slice, u64::MAX).await.unwrap(),
        };
        assert_eq!(read, N as u64);
        array
    }

    #[tokio::test]
    async fn sign() {
        let keys = root(Ledger::new()).await.unwrap().open_dir().await.unwrap();

        // Generate a key.
        let mut generate = open_file(&*keys, "generate", true, true).await;
        write(&mut *generate, &[ES256], false).await.unwrap();

        // Read the UUID.
        let uuid: [u8; 36] = read(&mut *generate, false).await;
        let uuid: Uuid = std::str::from_utf8(&uuid).unwrap().parse().unwrap();

        // Ensure the new key appears in the directory listing.
        keys.readdir(0.into())
            .await
            .unwrap()
            .map(|e| e.unwrap().name)
            .find(|x| &uuid.as_hyphenated().to_string() == x)
            .unwrap();

        // Open the share file.
        let mut share = open_file(&*keys, &format!("{}/share", uuid), true, false).await;

        // Export the public key.
        let pubkey: [u8; 69] = read(&mut *share, false).await;
        let pubkey = p256::PublicKey::from_sec1_bytes(&pubkey[4..]).unwrap();

        // Open the sign socket.
        let mut sign = open_file(&*keys, &format!("{}/sign", uuid), true, true).await;

        // Sign the message.
        write(&mut *sign, &[b"foo"], false).await.unwrap();
        let signature: [u8; 64] = read(&mut *sign, true).await;

        // Verify the signature.
        let vkey = p256::ecdsa::VerifyingKey::from(pubkey);
        let sig = p256::ecdsa::Signature::from_bytes(&signature).unwrap();
        vkey.verify(b"foo", &sig).unwrap();
    }

    #[tokio::test]
    async fn verify() {
        let sk = p256::ecdsa::SigningKey::random(&mut rand::thread_rng());
        let ep = p256::ecdsa::VerifyingKey::from(&sk).to_encoded_point(false);
        let mut sig = sk.sign(b"foo").as_bytes().to_vec();

        let keys = root(Ledger::new()).await.unwrap().open_dir().await.unwrap();

        // Trust the key.
        let mut trust = open_file(&*keys, "trust", true, true).await;
        write(&mut *trust, &[ES256, ep.as_bytes()], false)
            .await
            .unwrap();

        // Read the UUID.
        let uuid: [u8; 36] = read(&mut *trust, false).await;
        let uuid: Uuid = std::str::from_utf8(&uuid).unwrap().parse().unwrap();

        // Ensure the new key appears in the directory listing.
        keys.readdir(0.into())
            .await
            .unwrap()
            .map(|e| e.unwrap().name)
            .find(|x| &uuid.as_hyphenated().to_string() == x)
            .unwrap();

        // Open the verify socket.
        let mut verify = open_file(&*keys, &format!("{}/verify", uuid), false, true).await;

        // Verify the message.
        write(&mut *verify, &[b"foo"], false).await.unwrap();
        write(&mut *verify, &[&sig], true).await.unwrap();

        // Check that a bad signature fails.
        sig[0] += 1;
        write(&mut *verify, &[b"foo"], false).await.unwrap();
        let error = write(&mut *verify, &[&sig], true).await.unwrap_err();

        // Work around the lack of Eq on ErrorKind.
        // See: https://github.com/bytecodealliance/wasmtime/pull/5006
        assert_eq!(
            std::mem::discriminant(&ErrorKind::Ilseq),
            std::mem::discriminant(&error.downcast::<ErrorKind>().unwrap())
        );
    }

    #[tokio::test]
    async fn remove() {
        let keys = root(Ledger::new()).await.unwrap().open_dir().await.unwrap();

        // Generate a key.
        let mut generate = open_file(&*keys, "generate", true, true).await;
        write(&mut *generate, &[ES256], false).await.unwrap();

        // Read the UUID.
        let uuid: [u8; 36] = read(&mut *generate, false).await;
        let uuid: Uuid = std::str::from_utf8(&uuid).unwrap().parse().unwrap();

        // Ensure the new key appears in the directory listing.
        keys.readdir(0.into())
            .await
            .unwrap()
            .map(|e| e.unwrap().name)
            .find(|x| &uuid.as_hyphenated().to_string() == x)
            .unwrap();

        // Remove the key.
        keys.remove_dir(&uuid.to_string()).await.unwrap();

        // Ensure the key does not appear in the directory listing.
        let found = keys
            .readdir(0.into())
            .await
            .unwrap()
            .map(|e| e.unwrap().name)
            .any(|x| uuid.as_hyphenated().to_string() == x);
        assert!(!found);
    }
}
