use std::any::Any;
use std::io::{IoSlice, IoSliceMut};
use std::sync::Arc;

use digest::Digest;

use ecdsa::elliptic_curve::sec1::{FromEncodedPoint, ModulusSize, ToEncodedPoint};

use ecdsa::elliptic_curve::{AffinePoint, FieldSize, ProjectiveArithmetic};

use ecdsa::PrimeCurve;
use rsa::PublicKeyParts;
use sha2::{Sha256, Sha384, Sha512};
use signature::{DigestVerifier, Signature};
use uuid::Uuid;
use wasi_common::file::{FdFlags, FileType, Filestat, RiFlags, RoFlags, SiFlags};
use wasi_common::{Error, ErrorExt, ErrorKind, WasiDir, WasiFile};
use wasmtime_vfs_dir::Directory;
use wasmtime_vfs_ledger::InodeId;
use wasmtime_vfs_memory::{Data, Inode, Link, Node};

use crate::share::Share;
use crate::verify::Verify;
use crate::{ES256, ES256K, ES384, PS256, PS384, PS512, RS256, RS384, RS512};

type Rs256 = rsa::pkcs1v15::VerifyingKey<Sha256>;
type Rs384 = rsa::pkcs1v15::VerifyingKey<Sha384>;
type Rs512 = rsa::pkcs1v15::VerifyingKey<Sha512>;
type Ps256 = rsa::pss::VerifyingKey<Sha256>;
type Ps384 = rsa::pss::VerifyingKey<Sha384>;
type Ps512 = rsa::pss::VerifyingKey<Sha512>;
type Es256k = ecdsa::VerifyingKey<k256::Secp256k1>;
type Es256 = ecdsa::VerifyingKey<p256::NistP256>;
type Es384 = ecdsa::VerifyingKey<p384::NistP384>;

trait Decoder: Sized {
    fn decode(data: &[u8]) -> Result<Self, Error>;
}

impl Decoder for rsa::RsaPublicKey {
    fn decode(data: &[u8]) -> Result<Self, Error> {
        let mut el = [0; 4];
        let mut nl = [0; 4];

        if data.len() < 4 {
            return Err(Error::illegal_byte_sequence());
        }

        el.copy_from_slice(&data[..4]);
        let el = u32::from_be_bytes(el);

        if data.len() < 8 + el as usize {
            return Err(Error::illegal_byte_sequence());
        }

        nl.copy_from_slice(&data[4 + el as usize..][..4]);
        let nl = u32::from_be_bytes(nl);

        if data.len() != 8 + el as usize + nl as usize {
            return Err(Error::illegal_byte_sequence());
        }

        let e = rsa::BigUint::from_bytes_be(&data[4..][..el as usize]);
        let n = rsa::BigUint::from_bytes_be(&data[8 + el as usize..][..nl as usize]);

        let key = rsa::RsaPublicKey::new(n, e).map_err(|_| Error::illegal_byte_sequence())?;
        if key.size() * 8 < 2048 {
            return Err(Error::perm());
        }

        Ok(key)
    }
}

impl Decoder for Rs256 {
    fn decode(data: &[u8]) -> Result<Self, Error> {
        Ok(rsa::RsaPublicKey::decode(data)?.into())
    }
}

impl Decoder for Rs384 {
    fn decode(data: &[u8]) -> Result<Self, Error> {
        Ok(rsa::RsaPublicKey::decode(data)?.into())
    }
}

impl Decoder for Rs512 {
    fn decode(data: &[u8]) -> Result<Self, Error> {
        Ok(rsa::RsaPublicKey::decode(data)?.into())
    }
}

impl Decoder for Ps256 {
    fn decode(data: &[u8]) -> Result<Self, Error> {
        Ok(rsa::RsaPublicKey::decode(data)?.into())
    }
}

impl Decoder for Ps384 {
    fn decode(data: &[u8]) -> Result<Self, Error> {
        Ok(rsa::RsaPublicKey::decode(data)?.into())
    }
}

impl Decoder for Ps512 {
    fn decode(data: &[u8]) -> Result<Self, Error> {
        Ok(rsa::RsaPublicKey::decode(data)?.into())
    }
}

impl<C> Decoder for ecdsa::VerifyingKey<C>
where
    C: PrimeCurve + ProjectiveArithmetic,
    AffinePoint<C>: FromEncodedPoint<C> + ToEncodedPoint<C>,
    FieldSize<C>: ModulusSize,
{
    fn decode(data: &[u8]) -> Result<Self, Error> {
        Self::from_sec1_bytes(data).map_err(|_| Error::illegal_byte_sequence())
    }
}

pub struct Trust(Link<Vec<Uuid>>);

#[async_trait::async_trait]
impl Node for Trust {
    fn to_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
        self
    }

    fn parent(&self) -> Option<Arc<dyn Node>> {
        self.0.parent.upgrade()
    }

    fn filetype(&self) -> FileType {
        FileType::SocketDgram
    }

    fn id(&self) -> Arc<InodeId> {
        self.0.inode.id.clone()
    }

    async fn open_dir(self: Arc<Self>) -> Result<Box<dyn WasiDir>, Error> {
        Err(Error::not_dir())
    }

    async fn open_file(
        self: Arc<Self>,
        _path: &str,
        dir: bool,
        read: bool,
        write: bool,
        flags: FdFlags,
    ) -> Result<Box<dyn WasiFile>, Error> {
        if dir {
            return Err(Error::not_dir());
        }

        if !read || !write {
            return Err(Error::perm()); // FIXME: errno
        }

        if !flags.is_empty() {
            return Err(Error::invalid_argument()); // FIXME: errno
        }

        Ok(Box::new(OpenTrust {
            _root: self.root(),
            link: self,
        }))
    }
}

impl Trust {
    pub fn new(parent: Arc<dyn Node>) -> Arc<Self> {
        let id = parent.id().device().create_inode();

        let inode = Inode {
            data: Data::from(Vec::new()).into(),
            id,
        };

        Arc::new(Self(Link {
            parent: Arc::downgrade(&parent),
            inode: inode.into(),
        }))
    }

    async fn add<T, D, S>(self: &Arc<Trust>, bytes: &[u8]) -> Result<Uuid, Error>
    where
        T: Send + Sync + 'static,
        D: Send + Sync + 'static,
        S: Send + Sync + 'static,
        T: DigestVerifier<D, S> + Decoder,
        D: Digest + Clone,
        S: Signature,
    {
        let parent = self
            .parent()
            .ok_or_else(Error::io)?
            .to_any()
            .downcast::<Directory>()
            .map_err(|_| Error::io())?;

        let public = T::decode(&bytes[4..])?;
        let uuid = uuid::Uuid::new_v4();

        let d = Directory::new(parent.clone());
        d.attach("verify", Verify::new(d.clone(), public)).await?;
        d.attach("share", Share::new(d.clone(), bytes)).await?;
        parent.attach(&uuid.to_string(), d).await?;

        Ok(uuid)
    }
}

struct OpenTrust {
    _root: Arc<dyn Node>,
    link: Arc<Trust>,
}

#[async_trait::async_trait]
impl WasiFile for OpenTrust {
    fn as_any(&self) -> &dyn Any {
        self
    }

    async fn get_filetype(&mut self) -> Result<FileType, Error> {
        Ok(FileType::SocketDgram)
    }

    async fn get_filestat(&mut self) -> Result<Filestat, Error> {
        let ilock = self.link.0.inode.data.read().await;

        Ok(Filestat {
            device_id: **self.link.0.inode.id.device(),
            inode: **self.link.0.inode.id,
            filetype: FileType::SocketDgram,
            nlink: Arc::strong_count(&self.link.0.inode) as u64,
            size: 0,
            atim: Some(ilock.access),
            mtim: Some(ilock.modify),
            ctim: Some(ilock.create),
        })
    }

    async fn sock_recv<'a>(
        &mut self,
        bufs: &mut [IoSliceMut<'a>],
        _flags: RiFlags,
    ) -> Result<(u64, RoFlags), Error> {
        let n = self.read_vectored(bufs).await?;
        Ok((n, RoFlags::empty()))
    }

    async fn sock_send<'a>(&mut self, bufs: &[IoSlice<'a>], _flags: SiFlags) -> Result<u64, Error> {
        self.write_vectored(bufs).await
    }

    async fn read_vectored<'a>(&mut self, bufs: &mut [IoSliceMut<'a>]) -> Result<u64, Error> {
        let mut ilock = self.link.0.inode.data.write().await;

        if let Some(uuid) = ilock.content.pop() {
            let name = uuid.to_string();
            let bytes = name.as_bytes();
            let mut total = 0;

            for buf in bufs {
                let len = std::cmp::min(buf.len(), bytes.len() - total);
                buf[..len].copy_from_slice(&bytes[total..][..len]);
                total += len;
            }

            if total < bytes.len() {
                ilock.content.push(uuid);
                return Err(Error::too_big());
            }

            return Ok(total as u64);
        }

        Err(ErrorKind::WouldBlk.into())
    }

    async fn write_vectored<'a>(&mut self, bufs: &[IoSlice<'a>]) -> Result<u64, Error> {
        match bufs.iter().map(|x| x.len()).sum() {
            4..=4096 => {
                let mut all = Vec::with_capacity(4096);
                for buf in bufs {
                    all.extend_from_slice(buf);
                }

                let uuid = match &all[..4] {
                    RS256 => self.link.add::<Rs256, _, _>(&all).await?,
                    RS384 => self.link.add::<Rs384, _, _>(&all).await?,
                    RS512 => self.link.add::<Rs512, _, _>(&all).await?,
                    PS256 => self.link.add::<Ps256, _, _>(&all).await?,
                    PS384 => self.link.add::<Ps384, _, _>(&all).await?,
                    PS512 => self.link.add::<Ps512, _, _>(&all).await?,
                    ES256K => self.link.add::<Es256k, Sha256, _>(&all).await?,
                    ES256 => self.link.add::<Es256, Sha256, _>(&all).await?,
                    ES384 => self.link.add::<Es384, Sha384, _>(&all).await?,
                    _ => return Err(ErrorKind::Ilseq.into()),
                };

                self.link.0.inode.data.write().await.content.push(uuid);
                Ok(all.len() as u64)
            }

            _ => Err(Error::invalid_argument()),
        }
    }

    async fn readable(&self) -> Result<(), Error> {
        Ok(())
    }

    async fn writable(&self) -> Result<(), Error> {
        Ok(())
    }
}
