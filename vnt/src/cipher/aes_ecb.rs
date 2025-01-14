use crate::cipher::Finger;
use crate::protocol::body::AesCbcSecretBody;
use crate::protocol::{NetPacket, HEAD_LEN};
use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, BlockEncryptMut, KeyInit};
use rand::RngCore;
use std::io;

type Aes128EcbEnc = ecb::Encryptor<aes::Aes128>;
type Aes128EcbDec = ecb::Decryptor<aes::Aes128>;
type Aes256EcbEnc = ecb::Encryptor<aes::Aes256>;
type Aes256EcbDec = ecb::Decryptor<aes::Aes256>;

#[derive(Clone)]
pub struct AesEcbCipher {
    pub(crate) cipher: AesEcbEnum,
    pub(crate) finger: Option<Finger>,
}

#[derive(Clone)]
pub enum AesEcbEnum {
    AES128ECB([u8; 16]),
    AES256ECB([u8; 32]),
}

impl AesEcbCipher {
    pub fn key(&self) -> &[u8] {
        match &self.cipher {
            AesEcbEnum::AES128ECB(key) => key,
            AesEcbEnum::AES256ECB(key) => key,
        }
    }
}

impl AesEcbCipher {
    pub fn new_128(key: [u8; 16], finger: Option<Finger>) -> Self {
        Self {
            cipher: AesEcbEnum::AES128ECB(key),
            finger,
        }
    }
    pub fn new_256(key: [u8; 32], finger: Option<Finger>) -> Self {
        Self {
            cipher: AesEcbEnum::AES256ECB(key),
            finger,
        }
    }

    pub fn decrypt_ipv4<B: AsRef<[u8]> + AsMut<[u8]>>(
        &self,
        net_packet: &mut NetPacket<B>,
    ) -> io::Result<()> {
        if !net_packet.is_encrypt() {
            //未加密的数据直接丢弃
            return Err(io::Error::new(io::ErrorKind::Other, "not encrypt"));
        }
        if net_packet.payload().len() < 16 {
            log::error!("数据异常,长度{}小于{}", net_packet.payload().len(), 16);
            return Err(io::Error::new(io::ErrorKind::Other, "data err"));
        }
        let mut iv = [0; 16];
        iv[0..4].copy_from_slice(&net_packet.source().octets());
        iv[4..8].copy_from_slice(&net_packet.destination().octets());
        iv[8] = net_packet.protocol().into();
        iv[9] = net_packet.transport_protocol();
        iv[10] = net_packet.is_gateway() as u8;
        iv[11] = net_packet.source_ttl();
        if let Some(finger) = &self.finger {
            iv[12..16].copy_from_slice(&finger.hash[0..4]);
        }

        let mut secret_body =
            AesCbcSecretBody::new(net_packet.payload_mut(), self.finger.is_some())?;
        if let Some(finger) = &self.finger {
            let finger = finger.calculate_finger(&iv[..12], secret_body.en_body());
            if &finger != secret_body.finger() {
                return Err(io::Error::new(io::ErrorKind::Other, "finger err"));
            }
        }
        let rs = match &self.cipher {
            AesEcbEnum::AES128ECB(key) => Aes128EcbDec::new(&(*key).into())
                .decrypt_padded_mut::<Pkcs7>(secret_body.en_body_mut()),
            AesEcbEnum::AES256ECB(key) => Aes256EcbDec::new(&(*key).into())
                .decrypt_padded_mut::<Pkcs7>(secret_body.en_body_mut()),
        };
        match rs {
            Ok(buf) => {
                let len = buf.len();
                net_packet.set_encrypt_flag(false);
                //减去末尾的随机数
                net_packet.set_data_len(HEAD_LEN + len - 4)?;
                Ok(())
            }
            Err(e) => Err(io::Error::new(
                io::ErrorKind::Other,
                format!("解密失败:{}", e),
            )),
        }
    }
    /// net_packet 必须预留足够长度
    /// data_len是有效载荷的长度
    pub fn encrypt_ipv4<B: AsRef<[u8]> + AsMut<[u8]>>(
        &self,
        net_packet: &mut NetPacket<B>,
    ) -> io::Result<()> {
        let data_len = net_packet.data_len();
        let mut iv = [0; 16];
        iv[0..4].copy_from_slice(&net_packet.source().octets());
        iv[4..8].copy_from_slice(&net_packet.destination().octets());
        iv[8] = net_packet.protocol().into();
        iv[9] = net_packet.transport_protocol();
        iv[10] = net_packet.is_gateway() as u8;
        iv[11] = net_packet.source_ttl();
        if let Some(finger) = &self.finger {
            iv[12..16].copy_from_slice(&finger.hash[0..4]);
            net_packet.set_data_len(data_len + 16)?;
        } else {
            net_packet.set_data_len(data_len + 4)?;
        }
        //先扩充随机数

        let mut secret_body =
            AesCbcSecretBody::new(net_packet.payload_mut(), self.finger.is_some())?;
        secret_body.set_random(rand::thread_rng().next_u32());
        let p_len = secret_body.en_body().len();
        net_packet.set_data_len_max();
        let rs = match &self.cipher {
            AesEcbEnum::AES128ECB(key) => Aes128EcbEnc::new(&(*key).into())
                .encrypt_padded_mut::<Pkcs7>(net_packet.payload_mut(), p_len),
            AesEcbEnum::AES256ECB(key) => Aes256EcbEnc::new(&(*key).into())
                .encrypt_padded_mut::<Pkcs7>(net_packet.payload_mut(), p_len),
        };
        return match rs {
            Ok(buf) => {
                let len = buf.len();
                if let Some(finger) = &self.finger {
                    let finger = finger.calculate_finger(&iv[..12], buf);
                    //设置实际长度
                    net_packet.set_data_len(HEAD_LEN + len + finger.len())?;
                    let mut secret_body = AesCbcSecretBody::new(net_packet.payload_mut(), true)?;
                    secret_body.set_finger(&finger)?;
                } else {
                    net_packet.set_data_len(HEAD_LEN + len)?;
                }

                net_packet.set_encrypt_flag(true);
                Ok(())
            }
            Err(e) => Err(io::Error::new(
                io::ErrorKind::Other,
                format!("加密失败:{}", e),
            )),
        };
    }
}
