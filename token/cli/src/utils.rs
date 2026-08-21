use sha2::{Digest, Sha256};
use solana_sdk::{
    pubkey::Pubkey, signature::{Signature, Signer}, 
};
use serde_json::Value;
use crate::{clap_app::Error};

pub const ONS_PROGRAM_ID: &str = "on6LJ2wZa2jRAdnouvPtkAxLtZfVr9y8J7dMLgeDWLg";
pub const ONS_API_URL: &str = "http://192.168.204.128:5056";
pub const VOUCHER_PROGRAM_ID: &str = "vokWjyeSruKu2xoZkYNTz9fgW5Ba1rrrmzbXSQs3j59";
pub fn hash_name(name: &str) -> [u8; 16] {
    let mut hasher = Sha256::new();
    hasher.update(name.as_bytes());
    let hash = hasher.finalize();
    let mut result = [0u8; 16];
    result.copy_from_slice(&hash[..16]);
    result
}

pub fn find_pda(seeds: &[&[u8]], program_id: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(seeds, program_id)
}

#[derive(serde::Deserialize)]
pub struct TreasuryConfig {
    pub fee_receiver: String,
    pub fee_shortname: u64,
    pub fee_longname: u64,
}

#[derive(serde::Deserialize)]
pub struct VoucherTreasuryConfig {
    pub fee_receiver: String,
    pub fees: u64,
}

#[derive(serde::Deserialize, Clone)]
pub struct MetaConfig {
    pub fee_receiver: String,
    pub fee_shortname: u64,
    pub fee_longname: u64,
    pub basis_points_sell: u64,
    pub basis_points_rent: u64,
    #[serde(default)] 
    pub pay_token: Option<String>,
}

pub fn instruction_discriminator(name: &str) -> [u8; 8] {
    let mut hasher = Sha256::new();
    hasher.update(b"global:");
    hasher.update(name.as_bytes());
    let hash = hasher.finalize();
    let mut result = [0u8; 8];
    result.copy_from_slice(&hash[..8]);
    result
}

#[derive(serde::Serialize, Debug)]
pub struct HeaderInfoJson {
    pub name: String,
    pub owner: String,
    pub owner_start: i64,
    pub owner_end: i64,
    pub sell_enabled: u8,
    pub sell_price: u64,
    pub user: String,
    pub user_start: i64,
    pub user_end: i64,
    pub rent_enabled: u8,
    pub rent_per_day: u64,
}

impl HeaderInfoJson {
    pub fn deserialize(data: &[u8]) -> Option<Self> {
        let mut idx = 8; // Skip 8-byte discriminator (Anchor)

        if data.len() < idx + 4 {
            return None;
        }
        let name_len = u32::from_le_bytes([data[idx], data[idx+1], data[idx+2], data[idx+3]]) as usize;
        idx += 4;

        if data.len() < idx + name_len {
            return None;
        }
        let name = String::from_utf8(data[idx..idx + name_len].to_vec()).ok()?;
        idx += name_len;

        if data.len() < idx + 32 {
            return None;
        }
        let owner = Pubkey::new_from_array((&data[idx..idx + 32]).try_into().ok()?);
        idx += 32;

        let owner_start = i64::from_le_bytes(data[idx..idx+8].try_into().ok()?); idx += 8;
        let owner_end = i64::from_le_bytes(data[idx..idx+8].try_into().ok()?); idx += 8;

        let sell_enabled = data[idx]; idx += 1;
        let sell_price = u64::from_le_bytes(data[idx..idx+8].try_into().ok()?); idx += 8;

        let user = Pubkey::new_from_array((&data[idx..idx + 32]).try_into().ok()?); idx += 32;

        let user_start = i64::from_le_bytes(data[idx..idx+8].try_into().ok()?); idx += 8;
        let user_end = i64::from_le_bytes(data[idx..idx+8].try_into().ok()?); idx += 8;

        let rent_enabled = data[idx]; idx += 1;
        let rent_per_day = u64::from_le_bytes(data[idx..idx+8].try_into().ok()?);

        Some(HeaderInfoJson {
            name,
            owner: owner.to_string(),
            owner_start,
            owner_end,
            sell_enabled,
            sell_price,
            user: user.to_string(),
            user_start,
            user_end,
            rent_enabled,
            rent_per_day,
        })
    }
}

#[derive(serde::Serialize, Debug)]
pub struct AuthJsonConfigJson {
    pub authority: String,
    pub config_json: String,
}

impl AuthJsonConfigJson {
    pub fn deserialize(data: &[u8]) -> Option<Self> {
        let mut idx = 8; // Skip discriminator

        if data.len() < idx + 32 {
            return None;
        }
        let authority = Pubkey::new_from_array((&data[idx..idx + 32]).try_into().ok()?);
        idx += 32;

        if data.len() < idx + 4 {
            return None;
        }
        let json_len = u32::from_le_bytes([data[idx], data[idx+1], data[idx+2], data[idx+3]]) as usize;
        idx += 4;

        if data.len() < idx + json_len {
            return None;
        }
        let config_json = String::from_utf8(data[idx..idx + json_len].to_vec()).ok()?;

        Some(AuthJsonConfigJson {
            authority: authority.to_string(),
            config_json,
        })
    }
    pub fn deserializee(data: &[u8]) -> Option<Value> {
        let mut idx = 8; // Skip discriminator

        if data.len() < idx + 32 {
            return None;
        }
        idx += 32;

        if data.len() < idx + 4 {
            return None;
        }
        let json_len = u32::from_le_bytes([data[idx], data[idx+1], data[idx+2], data[idx+3]]) as usize;
        idx += 4;

        if data.len() < idx + json_len {
            return None;
        }
        let config_json_str = String::from_utf8(data[idx..idx + json_len].to_vec()).ok()?;
        let json_value: Value = match serde_json::from_str(&config_json_str) {
            Ok(val) => val,                   
            Err(_) => Value::String(config_json_str),
        };
        Some(json_value)
    }
}

#[derive(Debug)]
pub struct VoucherDetailCli {
    pub code: Pubkey,
    pub mint: Pubkey,
    pub quota: u64,
    pub creator: Pubkey,
    pub create_time: i64,
    pub redeem_time: i64,
    pub redeemer: Pubkey,
}

pub fn parse_voucher_detail(
    data: &[u8],
) -> Result<VoucherDetailCli, Error> {

    // discriminator 8 bytes
    const HEADER: usize = 8;

    // 8 + 32 + 32 + 8 + 32 + 8 + 8 + 32
    const SIZE: usize =
        8 + 32 + 32 + 8 + 32 + 8 + 8 + 32;

    if data.len() < SIZE {
        return Err(
            format!(
                "VoucherDetail account data too short: {} < {}",
                data.len(),
                SIZE
            )
            .into()
        );
    }

    let mut offset = HEADER;

    // --------------------------------------------------------
    // code
    // --------------------------------------------------------

    let code =
        Pubkey::new_from_array(
            data[offset..offset + 32]
                .try_into()
                .map_err(|_| {
                    "Invalid code pubkey"
                })?
        );

    offset += 32;

    // --------------------------------------------------------
    // mint
    // --------------------------------------------------------

    let mint =
        Pubkey::new_from_array(
            data[offset..offset + 32]
                .try_into()
                .map_err(|_| {
                    "Invalid mint pubkey"
                })?
        );

    offset += 32;

    // --------------------------------------------------------
    // quota
    // --------------------------------------------------------

    let quota =
        u64::from_le_bytes(
            data[offset..offset + 8]
                .try_into()
                .map_err(|_| {
                    "Invalid quota"
                })?
        );

    offset += 8;

    // --------------------------------------------------------
    // creator
    // --------------------------------------------------------

    let creator =
        Pubkey::new_from_array(
            data[offset..offset + 32]
                .try_into()
                .map_err(|_| {
                    "Invalid creator pubkey"
                })?
        );

    offset += 32;

    // --------------------------------------------------------
    // create_time
    // --------------------------------------------------------

    let create_time =
        i64::from_le_bytes(
            data[offset..offset + 8]
                .try_into()
                .map_err(|_| {
                    "Invalid create_time"
                })?
        );

    offset += 8;

    // --------------------------------------------------------
    // redeem_time
    // --------------------------------------------------------

    let redeem_time =
        i64::from_le_bytes(
            data[offset..offset + 8]
                .try_into()
                .map_err(|_| {
                    "Invalid redeem_time"
                })?
        );

    offset += 8;

    // --------------------------------------------------------
    // redeemer
    // --------------------------------------------------------

    let redeemer =
        Pubkey::new_from_array(
            data[offset..offset + 32]
                .try_into()
                .map_err(|_| {
                    "Invalid redeemer pubkey"
                })?
        );

    Ok(
        VoucherDetailCli {
            code,
            mint,
            quota,
            creator,
            create_time,
            redeem_time,
            redeemer,
        }
    )
}

#[derive(Debug)]
pub struct VoucherResult {
    pub index: u64,
    pub public_key: Pubkey,
    pub redeem_code: String,
    pub signature: Signature,
}