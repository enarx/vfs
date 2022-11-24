use std::any::Any;
use std::io::{IoSlice, IoSliceMut};
use std::sync::Arc;

use digest::generic_array::ArrayLength;
use digest::Digest;
use ecdsa::elliptic_curve::ops::{Invert, Reduce};
use ecdsa::elliptic_curve::subtle::CtOption;
use ecdsa::elliptic_curve::{ProjectiveArithmetic, Scalar};
use ecdsa::hazmat::SignPrimitive;
use ecdsa::{PrimeCurve, SignatureSize};
use rsa::PublicKeyParts;
use sha2::{Sha256, Sha384, Sha512};
use signature::{DigestVerifier, RandomizedDigestSigner, Signature};
use uuid::Uuid;
use wasi_common::file::{FdFlags, FileType, Filestat, RiFlags, RoFlags, SiFlags};
use wasi_common::{Error, ErrorExt, ErrorKind, WasiDir, WasiFile};
use wasmtime_vfs_dir::Directory;
use wasmtime_vfs_ledger::InodeId;
use wasmtime_vfs_memory::{Data, Inode, Link, Node};

use crate::share::Share;
use crate::sign::Sign;
use crate::verify::Verify;
use crate::{ES256, ES256K, ES384, PS256, PS384, PS512, RS256, RS384, RS512};

type Rs256 = rsa::pkcs1v15::SigningKey<Sha256>;
type Rs384 = rsa::pkcs1v15::SigningKey<Sha384>;
type Rs512 = rsa::pkcs1v15::SigningKey<Sha512>;
type Ps256 = rsa::pss::BlindedSigningKey<Sha256>;
type Ps384 = rsa::pss::BlindedSigningKey<Sha384>;
type Ps512 = rsa::pss::BlindedSigningKey<Sha512>;
type Es256k = ecdsa::SigningKey<k256::Secp256k1>;
type Es256 = ecdsa::SigningKey<p256::NistP256>;
type Es384 = ecdsa::SigningKey<p384::NistP384>;

trait GenerateKey: Sized {
    fn generate() -> Result<Self, Error>;
}

impl GenerateKey for Rs256 {
    fn generate() -> Result<Self, Error> {
        Ok(rsa::RsaPrivateKey::new(&mut rand::thread_rng(), 2048)?.into())
    }
}

impl GenerateKey for Rs384 {
    fn generate() -> Result<Self, Error> {
        Ok(rsa::RsaPrivateKey::new(&mut rand::thread_rng(), 3072)?.into())
    }
}

impl GenerateKey for Rs512 {
    fn generate() -> Result<Self, Error> {
        Ok(rsa::RsaPrivateKey::new(&mut rand::thread_rng(), 4096)?.into())
    }
}

impl GenerateKey for Ps256 {
    fn generate() -> Result<Self, Error> {
        Ok(rsa::RsaPrivateKey::new(&mut rand::thread_rng(), 2048)?.into())
    }
}

impl GenerateKey for Ps384 {
    fn generate() -> Result<Self, Error> {
        Ok(rsa::RsaPrivateKey::new(&mut rand::thread_rng(), 3072)?.into())
    }
}

impl GenerateKey for Ps512 {
    fn generate() -> Result<Self, Error> {
        Ok(rsa::RsaPrivateKey::new(&mut rand::thread_rng(), 4096)?.into())
    }
}

impl GenerateKey for Es256k {
    fn generate() -> Result<Self, Error> {
        Ok(ecdsa::SigningKey::random(&mut rand::thread_rng()))
    }
}

impl GenerateKey for Es256 {
    fn generate() -> Result<Self, Error> {
        Ok(ecdsa::SigningKey::random(&mut rand::thread_rng()))
    }
}

impl GenerateKey for Es384 {
    fn generate() -> Result<Self, Error> {
        Ok(ecdsa::SigningKey::random(&mut rand::thread_rng()))
    }
}

trait ToPublic {
    type Public;

    fn to_public(&self) -> Self::Public;
}

impl<D: Digest> ToPublic for rsa::pkcs1v15::SigningKey<D> {
    type Public = rsa::pkcs1v15::VerifyingKey<D>;

    fn to_public(&self) -> Self::Public {
        Self::Public::from(self)
    }
}

impl<D: Digest> ToPublic for rsa::pss::BlindedSigningKey<D> {
    type Public = rsa::pss::VerifyingKey<D>;

    fn to_public(&self) -> Self::Public {
        Self::Public::from(self)
    }
}

impl<C: ecdsa::elliptic_curve::Curve> ToPublic for ecdsa::SigningKey<C>
where
    C: PrimeCurve + ProjectiveArithmetic,
    Scalar<C>: Invert<Output = CtOption<Scalar<C>>> + Reduce<C::UInt> + SignPrimitive<C>,
    SignatureSize<C>: ArrayLength<u8>,
{
    type Public = ecdsa::VerifyingKey<C>;

    fn to_public(&self) -> Self::Public {
        Self::Public::from(self)
    }
}

trait Encoder<T> {
    fn encode(&self, arg: T) -> Result<Vec<u8>, Error>;
}

impl Encoder<&[u8]> for rsa::RsaPublicKey {
    fn encode(&self, prefix: &[u8]) -> Result<Vec<u8>, Error> {
        let e = self.e().to_bytes_be();
        let n = self.n().to_bytes_be();

        let el = u32::try_from(e.len()).map_err(|_| Error::io())?;
        let nl = u32::try_from(n.len()).map_err(|_| Error::io())?;

        let mut out = prefix.to_vec();
        out.extend_from_slice(&el.to_be_bytes());
        out.extend_from_slice(&e);
        out.extend_from_slice(&nl.to_be_bytes());
        out.extend_from_slice(&n);

        Ok(out)
    }
}

impl Encoder<()> for rsa::pkcs1v15::VerifyingKey<Sha256> {
    fn encode(&self, _: ()) -> Result<Vec<u8>, Error> {
        self.as_ref().encode(RS256)
    }
}

impl Encoder<()> for rsa::pkcs1v15::VerifyingKey<Sha384> {
    fn encode(&self, _: ()) -> Result<Vec<u8>, Error> {
        self.as_ref().encode(RS384)
    }
}

impl Encoder<()> for rsa::pkcs1v15::VerifyingKey<Sha512> {
    fn encode(&self, _: ()) -> Result<Vec<u8>, Error> {
        self.as_ref().encode(RS512)
    }
}

impl Encoder<()> for rsa::pss::VerifyingKey<Sha256> {
    fn encode(&self, _: ()) -> Result<Vec<u8>, Error> {
        self.as_ref().encode(PS256)
    }
}

impl Encoder<()> for rsa::pss::VerifyingKey<Sha384> {
    fn encode(&self, _: ()) -> Result<Vec<u8>, Error> {
        self.as_ref().encode(PS384)
    }
}

impl Encoder<()> for rsa::pss::VerifyingKey<Sha512> {
    fn encode(&self, _: ()) -> Result<Vec<u8>, Error> {
        self.as_ref().encode(PS512)
    }
}

impl Encoder<()> for ecdsa::VerifyingKey<k256::Secp256k1> {
    fn encode(&self, _: ()) -> Result<Vec<u8>, Error> {
        let mut out = ES256K.to_vec();
        out.extend_from_slice(self.to_encoded_point(false).as_bytes());
        Ok(out)
    }
}

impl Encoder<()> for ecdsa::VerifyingKey<p256::NistP256> {
    fn encode(&self, _: ()) -> Result<Vec<u8>, Error> {
        let mut out = ES256.to_vec();
        out.extend_from_slice(self.to_encoded_point(false).as_bytes());
        Ok(out)
    }
}

impl Encoder<()> for ecdsa::VerifyingKey<p384::NistP384> {
    fn encode(&self, _: ()) -> Result<Vec<u8>, Error> {
        let mut out = ES384.to_vec();
        out.extend_from_slice(self.to_encoded_point(false).as_bytes());
        Ok(out)
    }
}

pub struct Generate(Link<Vec<Uuid>>);

#[async_trait::async_trait]
impl Node for Generate {
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

        Ok(Box::new(OpenGenerate {
            _root: self.root(),
            link: self,
        }))
    }
}

impl Generate {
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

    async fn add<T, U, D, S>(self: &Arc<Generate>) -> Result<Uuid, Error>
    where
        T: Send + Sync + 'static,
        U: Send + Sync + 'static,
        D: Send + Sync + 'static,
        S: Send + Sync + 'static,
        T: RandomizedDigestSigner<D, S> + GenerateKey + ToPublic<Public = U>,
        U: DigestVerifier<D, S> + Encoder<()>,
        D: Digest + Clone,
        S: Signature,
    {
        let parent = self
            .parent()
            .ok_or_else(Error::io)?
            .to_any()
            .downcast::<Directory>()
            .map_err(|_| Error::io())?;

        let secret = T::generate()?;
        let public = secret.to_public();
        let shared = public.encode(())?;
        let uuid = uuid::Uuid::new_v4();

        let d = Directory::new(parent.clone(), None);
        d.attach("verify", Verify::new(d.clone(), public)).await?;
        d.attach("share", Share::new(d.clone(), shared)).await?;
        d.attach("sign", Sign::new(d.clone(), secret)).await?;
        parent.attach(&uuid.to_string(), d).await?;

        Ok(uuid)
    }
}

struct OpenGenerate {
    _root: Arc<dyn Node>,
    link: Arc<Generate>,
}

#[async_trait::async_trait]
impl WasiFile for OpenGenerate {
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
        if bufs.len() != 1 || bufs[0].len() != 4 {
            return Err(Error::invalid_argument());
        }

        let uuid = match bufs[0].as_ref() {
            RS256 => self.link.add::<Rs256, _, _, _>().await?,
            RS384 => self.link.add::<Rs384, _, _, _>().await?,
            RS512 => self.link.add::<Rs512, _, _, _>().await?,
            PS256 => self.link.add::<Ps256, _, _, _>().await?,
            PS384 => self.link.add::<Ps384, _, _, _>().await?,
            PS512 => self.link.add::<Ps512, _, _, _>().await?,
            ES256K => self.link.add::<Es256k, _, Sha256, _>().await?,
            ES256 => self.link.add::<Es256, _, Sha256, _>().await?,
            ES384 => self.link.add::<Es384, _, Sha384, _>().await?,
            _ => return Err(ErrorKind::Ilseq.into()),
        };

        self.link.0.inode.data.write().await.content.push(uuid);
        Ok(4)
    }

    async fn readable(&self) -> Result<(), Error> {
        Ok(())
    }

    async fn writable(&self) -> Result<(), Error> {
        Ok(())
    }
}
