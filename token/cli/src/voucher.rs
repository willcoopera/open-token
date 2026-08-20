/// The `entity` subcommand
use {
    crate::{clap_app::Error, command::CommandResult, config::Config}, clap::{value_t_or_exit, ArgMatches}, futures::future::ok, solana_clap_utils::input_parsers::pubkey_of_signer, solana_client::{
        nonblocking::rpc_client::RpcClient, rpc_client::RpcClient as BlockingRpcClient,
        tpu_client::{TpuClient, TpuClientConfig},
    }, solana_remote_wallet::remote_wallet::RemoteWalletManager, 
    solana_sdk::{
        message::Message, native_token::{lamports_to_sol, Sol}, program_pack::Pack, pubkey::Pubkey, 
        signature::{Signature, Signer}, signer, system_instruction, instruction::{AccountMeta, Instruction}, transaction::Transaction,
        compute_budget::ComputeBudgetInstruction,
    }, 
    spl_associated_token_account::*, spl_token_2022::{
        extension::StateWithExtensions,
        instruction,
        state::{Account, Mint},
    }, std::{rc::Rc, sync::Arc, time::Instant
    }, crate::utils::{hash_name, find_pda, ONS_PROGRAM_ID, VoucherTreasuryConfig as TreasuryConfig, MetaConfig, instruction_discriminator, 
        HeaderInfoJson, AuthJsonConfigJson, ONS_API_URL,VOUCHER_PROGRAM_ID,
    }, solana_program::{pubkey::Pubkey as SolPubkey
    }, std::str::FromStr,
    serde_json::Value,
    spl_associated_token_account::{ error::AssociatedTokenAccountError, get_associated_token_address_with_program_id,
        instruction::{create_associated_token_account,},
    },
    bs58,
    rust_xlsxwriter::{Format, Workbook},
    chrono::Local,
};

#[derive(Debug)]
struct VoucherResult {
    index: u64,
    public_key: Pubkey,
    redeem_code: String,
    signature: Signature,
}

#[derive(Debug)]
struct VoucherDetailCli {
    pub code: Pubkey,
    pub mint: Pubkey,
    pub quota: u64,
    pub creator: Pubkey,
    pub create_time: i64,
    pub redeem_time: i64,
    pub redeemer: Pubkey,
}

fn parse_voucher_detail(
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

pub(crate) async fn voucher_process_command(
    matches: &ArgMatches<'_>,
    config: &Config<'_>,
    mut signers: Vec<Arc<dyn Signer>>,
    wallet_manager: &mut Option<Rc<RemoteWalletManager>>,
) -> CommandResult {
    assert!(!config.sign_only);

    match matches.subcommand() {
        ("create", Some(arg_matches)) => {
            let mint = value_t_or_exit!(arg_matches, "mint", String);
            let quota = value_t_or_exit!(arg_matches, "quota", u64);
            let count = value_t_or_exit!(arg_matches, "count", u64);
            let (owner_signer, owner) =
                config.signer_or_default_ons(arg_matches, "owner", wallet_manager);
            signers.push(owner_signer);
            let program_id = SolPubkey::from_str(VOUCHER_PROGRAM_ID).unwrap();
            command_create(config, signers, mint, quota, count, &owner, &program_id).await?;
        }
        ("redeem", Some(arg_matches)) => {
            let code = value_t_or_exit!(arg_matches, "code", String);
            let (owner_signer, owner) =
                config.signer_or_default_ons(arg_matches, "owner", wallet_manager);
            signers.push(owner_signer);
            let program_id = SolPubkey::from_str(VOUCHER_PROGRAM_ID).unwrap();
            command_redeem(config, signers, code, &owner, &program_id).await?;
        }
        ("transfer-ownership", Some(arg_matches)) => {
            let name = value_t_or_exit!(arg_matches, "name", String);
            let to_pubkey = pubkey_of_signer(arg_matches, "to", wallet_manager)
                .expect("Required argument 'to' missing or invalid");
            let (owner_signer, owner) =
                config.signer_or_default_ons(arg_matches, "owner", wallet_manager);
            signers.push(owner_signer);
            let program_id = SolPubkey::from_str(ONS_PROGRAM_ID).unwrap();
            command_transfer_ownership(config, signers, name, to_pubkey, &owner, &program_id).await?;
        }
        ("sale-ask", Some(arg_matches)) => {
            let name = value_t_or_exit!(arg_matches, "name", String);
            let enabled = value_t_or_exit!(arg_matches, "enabled", u8);
            let sell_price = value_t_or_exit!(arg_matches, "sell_price", u64);
            let (owner_signer, owner) =
                config.signer_or_default_ons(arg_matches, "owner", wallet_manager);
            signers.push(owner_signer);
            let program_id = SolPubkey::from_str(ONS_PROGRAM_ID).unwrap();
            command_sale_ask(config, signers, name, enabled, sell_price, &owner, &program_id).await?;
        }
        ("sale-bid", Some(arg_matches)) => {
            let name = value_t_or_exit!(arg_matches, "name", String);
            let (owner_signer, owner) =
                config.signer_or_default_ons(arg_matches, "owner", wallet_manager);
            signers.push(owner_signer);
            let program_id = SolPubkey::from_str(ONS_PROGRAM_ID).unwrap();
            command_sale_bid(config, signers, name, &owner, &program_id).await?;
        }
        ("rent-ask", Some(arg_matches)) => {
            let name = value_t_or_exit!(arg_matches, "name", String);
            let enabled = value_t_or_exit!(arg_matches, "enabled", u8);
            let rent_per_day = value_t_or_exit!(arg_matches, "rent_per_day", u64);
            let (owner_signer, owner) =
                config.signer_or_default_ons(arg_matches, "owner", wallet_manager);
            signers.push(owner_signer);
            let program_id = SolPubkey::from_str(ONS_PROGRAM_ID).unwrap();
            command_rent_ask(config, signers, name, enabled, rent_per_day, &owner, &program_id).await?;
        }
        ("rent-bid", Some(arg_matches)) => {
            let name = value_t_or_exit!(arg_matches, "name", String);
            let days = value_t_or_exit!(arg_matches, "days", u32);
            let (owner_signer, owner) =
                config.signer_or_default_ons(arg_matches, "owner", wallet_manager);
            signers.push(owner_signer);
            let program_id = SolPubkey::from_str(ONS_PROGRAM_ID).unwrap();
            command_rent_bid(config, signers, name, days, &owner, &program_id).await?;
        }
        ("update-rent", Some(arg_matches)) => {
            let name = value_t_or_exit!(arg_matches, "name", String);
            let rent_info = value_t_or_exit!(arg_matches, "rent-info", String);
            let (owner_signer, owner) =
                config.signer_or_default_ons(arg_matches, "owner", wallet_manager);
            signers.push(owner_signer);
            let program_id = SolPubkey::from_str(ONS_PROGRAM_ID).unwrap();
            command_update_rent(config, signers, name, rent_info, &owner, &program_id).await?;
        }
        ("renew-usership", Some(arg_matches)) => {
            let name = value_t_or_exit!(arg_matches, "name", String);
            let days = value_t_or_exit!(arg_matches, "days", u32);
            let (owner_signer, owner) =
                config.signer_or_default_ons(arg_matches, "owner", wallet_manager);
            signers.push(owner_signer);
            let program_id = SolPubkey::from_str(ONS_PROGRAM_ID).unwrap();
            command_renew_usership(config, signers, name, days, &owner, &program_id).await?;
        }
        ("transfer-usership", Some(arg_matches)) => {
            let name = value_t_or_exit!(arg_matches, "name", String);
            let to_pubkey = pubkey_of_signer(arg_matches, "to", wallet_manager)
                .expect("Required argument 'to' missing or invalid");
            let (owner_signer, owner) =
                config.signer_or_default_ons(arg_matches, "owner", wallet_manager);
            signers.push(owner_signer);
            let program_id = SolPubkey::from_str(ONS_PROGRAM_ID).unwrap();
            command_transfer_usership(config, signers, name, to_pubkey, &owner, &program_id).await?;
        }
        _ => unreachable!(),
    }

    Ok("".to_string())
}

fn keypair_from_base58(
    private_key: &str,
) -> Result<solana_sdk::signature::Keypair, Error> {

    let bytes =
        bs58::decode(
            private_key.trim()
        )
        .into_vec()
        .map_err(|e| {
            format!("Invalid redeem-code.")
        })?;

    if bytes.len() != 64 {
        return Err(
            format!(
                "Invalid redeem-code length: {}",
                bytes.len()
            )
            .into()
        );
    }

    let keypair =
        solana_sdk::signature::Keypair::from_bytes(
            &bytes
        )
        .map_err(|e| {
            format!(
                "Invalid redeem-code!"
            )
        })?;

    Ok(keypair)
}

async fn get_or_create_token_ata(
    rpc_client: &RpcClient,
    payer: &Pubkey,
    signers: &[Arc<dyn Signer>],
    owner: &Pubkey,
    mint: &Pubkey,
) -> Result<Pubkey, Error> {
    let token2022_program_pbk = Pubkey::from_str("Token9ADbPtdFC3PjxaohBLGw2pgZwofdcbj6Lyaw6c").unwrap();
    let ata = get_associated_token_address_with_program_id(
            owner,
            mint,
            &token2022_program_pbk,
        );

    match rpc_client.get_account(&ata).await {
        Ok(_) => {
            //println!( "ATA already exists: {}", ata);
            return Ok(ata);
            }
        Err(_) => {
        }
    }

    //println!("ATA does not exist, creating: {}",  ata);
    let create_ata_ix = create_associated_token_account(
            payer,
            owner,
            mint,
            &token2022_program_pbk,
        );

    let blockhash = rpc_client.get_latest_blockhash().await?;
    let mut transaction = Transaction::new_with_payer(&[create_ata_ix], Some(payer));
    transaction.sign(signers, blockhash);

    let simulation = rpc_client.simulate_transaction(&transaction).await?;

    if let Some(err) = simulation.value.err{
        println!("ATA creation simulation failed:");
        for log in simulation
            .value
            .logs
            .unwrap_or_default()
        {
            println!("{}", log);
        }
        return Err(format!("ATA creation simulation failed: {:?}", err).into());
    }

    let signature = rpc_client.send_and_confirm_transaction(&transaction).await?;
    println!("ATA created successfully. tx: {}", signature);    

    rpc_client
        .get_account(&ata)
        .await
        .map_err(|e| {
            println!("ATA creation transaction succeeded,but ATA {} does not exist: {}", ata, e)
        });

    Ok(ata)
}
async fn get_mint_decimals(rpc_client: &RpcClient, mint: &Pubkey) -> Result<u8, Error> {
    let supply = rpc_client
        .get_token_supply(mint)
        .await
        .map_err(|e| {
            format!(
                "Failed to get token supply for mint {}: {}",
                mint,
                e
            )
        })?;

    Ok(supply.decimals)
}

fn export_vouchers_to_excel(
    filename: &str,
    vouchers: &[VoucherResult],
    decimals: u8,
) -> Result<(), Error> {

    let mut workbook =
        Workbook::new();

    let worksheet =
        workbook.add_worksheet();


    // --------------------------------------------------------
    // Header format
    // --------------------------------------------------------

    let header_format =
        Format::new()
            .set_bold();


    let headers = [
        "index",
        "public_key",
        "redeem_code",
        "signature",
    ];


    // --------------------------------------------------------
    // Header
    // --------------------------------------------------------

    for (col, header)
        in headers.iter().enumerate()
    {
        worksheet.write_string_with_format(
            0,
            col as u16,
            *header,
            &header_format,
        )?;
    }


    // --------------------------------------------------------
    // Rows
    // --------------------------------------------------------

    for (index, voucher)
        in vouchers.iter().enumerate()
    {
        let row =
            (index + 1) as u32;


        // index
        worksheet.write_number(
            row,
            0,
            voucher.index as f64,
        )?;


        // code
        worksheet.write_string(
            row,
            1,
            voucher.public_key.to_string(),
        )?;


        // private key
        worksheet.write_string(
            row,
            2,
            &voucher.redeem_code,
        )?;

        // tx
        worksheet.write_string(
            row,
            3,
            voucher.signature.to_string(),
        )?;
    }


    // --------------------------------------------------------
    // Column width
    // --------------------------------------------------------

    worksheet.set_column_width(
        0,
        10.0,
    )?;

    worksheet.set_column_width(
        1,
        45.0,
    )?;

    worksheet.set_column_width(
        2,
        90.0,
    )?;

    worksheet.set_column_width(
        3,
        45.0,
    )?;

    worksheet.set_column_width(
        4,
        45.0,
    )?;

    worksheet.set_column_width(
        5,
        50.0,
    )?;

    worksheet.set_column_width(
        6,
        20.0,
    )?;

    worksheet.set_column_width(
        7,
        25.0,
    )?;

    worksheet.set_column_width(
        8,
        12.0,
    )?;

    worksheet.set_column_width(
        9,
        90.0,
    )?;
    workbook.save(filename)?;

    Ok(())
}

async fn command_create(
    config: &Config<'_>,
    signers: Vec<Arc<dyn Signer>>,
    mint: String,
    quota: u64,
    count: u64,
    payer_pbk: &Pubkey,
    program_id: &Pubkey,
) -> Result<(), Error> {
    let rpc_client = &config.rpc_client;
    let token2022_program_pbk = Pubkey::from_str("Token9ADbPtdFC3PjxaohBLGw2pgZwofdcbj6Lyaw6c").unwrap();
    if quota < 1 {
        return Err(format!("Quota must be >= 1").into());
    }  
    if count < 1 {
        return Err(format!("Count must be >= 1").into());
    }
    let mint_pubkey = Pubkey::from_str(&mint)
            .map_err(|_| {
                format!(
                    "Invalid mint address: {}",
                    mint
                )
            })?;
    println!("==========================================");
    println!("Voucher Create");
    println!("==========================================");
    println!("Operator : {}", payer_pbk);
    println!("Mint     : {}", mint_pubkey);
    println!("Quota    : {}", quota);
    println!("Count    : {}", count);
    let decimals = get_mint_decimals(rpc_client, &mint_pubkey)
        .await
        .map_err(|e| {
            format!(
                "Failed to get mint decimals: {}",
                e
            )
        })?;

    //println!("Mint decimals: {}", decimals);
    let quota_amount = spl_token::ui_amount_to_amount(quota as f64, decimals);
    //println!("Quota raw amount: {}", quota_amount);
    let (treasury_pubkey, _) = find_pda(&[b"treasury_config"], program_id);
    let treasury_account = rpc_client.get_account(&treasury_pubkey).await
         .map_err(|_| format!("Treasury config account does not exist: {}", treasury_pubkey))?;
    let treasury_data = &treasury_account.data;
    if treasury_data.len() < 40 {
        return Err(format!("Treasury account data too short").into());
    }    
    let json_data = &treasury_data[40..];    
    if json_data.len() < 4 {
        return Err(format!("Invalid treasury JSON data: too short").into());
    }    
    let json_len = u32::from_le_bytes([json_data[0], json_data[1], json_data[2], json_data[3]]) as usize;    
    if json_data.len() < 4 + json_len {
        return Err(format!("Invalid treasury JSON length").into());
    }    
    let json_str = std::str::from_utf8(&json_data[4..4 + json_len])
        .map_err(|_| format!("Invalid UTF-8 in treasury config_json"))?;
    let treasury_config: TreasuryConfig = serde_json::from_str(json_str)
        .map_err(|e| format!("Failed to parse treasury config: {}", e))?;
    let fee_receiver_pubkey = Pubkey::from_str(&treasury_config.fee_receiver)
        .map_err(|_| format!("Invalid fee_receiver in treasury config"))?;
    //println!("fee_receiver_pubkey: {}", &treasury_config.fees);

    let operator_token_account = get_or_create_token_ata(
            rpc_client,
            payer_pbk,
            &signers,
            payer_pbk,
            &mint_pubkey,
        )
        .await
        .map_err(|e| {
            format!(
                "Failed to get/create operator ATA: {}",
                e
            )
        })?;
    //println!("Operator ATA: {}", operator_token_account);
    let (vault_pubkey, vault_bump) = Pubkey::find_program_address(
            &[
                b"vault",
                payer_pbk.as_ref(),
                mint_pubkey.as_ref(),
            ],
            program_id,
        );

    //println!("Vault PDA: {} bump: {}", vault_pubkey, vault_bump);
    let vault_token_account = get_or_create_token_ata(
            rpc_client,
            payer_pbk,
            &signers,
            &vault_pubkey,
            &mint_pubkey,
        )
        .await
        .map_err(|e| {
            format!("Failed to get/create vault ATA: {}", e)
        })?;

    //println!("Vault ATA: {}", vault_token_account);
    let mut results: Vec<VoucherResult> = Vec::with_capacity(count as usize);
    for index in 0..count {
        println!();
        println!("========== [{}/{}] ==========", index + 1, count);
        let voucher_keypair = solana_sdk::signature::Keypair::new();
        let public_key = voucher_keypair.pubkey();
        let redeem_code = bs58::encode(voucher_keypair.to_bytes()).into_string();
        println!("Public_Key : {}", public_key);
        println!("Redeem_Code: {}", redeem_code);
        let (detail_pubkey, detail_bump) =
            Pubkey::find_program_address(
                &[b"vo", public_key.as_ref()],
                program_id,
            );

        println!("Detail PDA : {}", detail_pubkey);
        let accounts = vec![
            AccountMeta::new(detail_pubkey, false),
            AccountMeta::new(*payer_pbk, true),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
            AccountMeta::new_readonly(treasury_pubkey, false),
            AccountMeta::new(vault_pubkey, false),
            AccountMeta::new(fee_receiver_pubkey, false),
            AccountMeta::new_readonly(mint_pubkey, false),
            AccountMeta::new_readonly(token2022_program_pbk, false),
            AccountMeta::new(operator_token_account, false),
            AccountMeta::new(vault_token_account, false),
        ];

        let mut data = Vec::with_capacity(8 + 32 + 8);

        data.extend_from_slice(&instruction_discriminator("voucher_create"));
        data.extend_from_slice(public_key.as_ref());
        data.extend_from_slice(&quota_amount.to_le_bytes());

        let instruction = Instruction {
            program_id: *program_id,
            accounts,
            data,
        };

        let blockhash = rpc_client.get_latest_blockhash()
                .await
                .map_err(|e| {
                    format!(
                        "Failed to get blockhash: {}",
                        e
                    )
                })?;
        let compute_budget_instruction = ComputeBudgetInstruction::set_compute_unit_limit(400_000);
        let mut transaction = Transaction::new_with_payer(&[compute_budget_instruction, instruction], Some(payer_pbk));
        transaction.sign(&signers, blockhash);

        let simulation = rpc_client.simulate_transaction(&transaction)
                .await
                .map_err(|e| {
                    format!(
                        "Simulation RPC failed: {}",
                        e
                    )
                })?;

        if let Some(err) = simulation.value.err{
            println!();
            println!(
                "Voucher {} simulation failed:",
                index + 1
            );

            for log in simulation
                .value
                .logs
                .unwrap_or_default()
            {
                println!("{}", log);
            }

            return Err(
                format!(
                    "Simulation failed: {:?}",
                    err
                )
                .into()
            );
        }

        let signature = rpc_client.send_and_confirm_transaction(&transaction)
                .await
                .map_err(|e| {
                    format!(
                        "Voucher {} send failed: {}",
                        index + 1,
                        e
                    )
                })?;

        println!("SUCCESS tx : {}", signature);
        results.push(
            VoucherResult {
                index: index + 1,
                public_key,
                redeem_code,
                signature,
            }
        );
    }
    let filename =
        format!(
            "voucher_{}.xlsx",
            Local::now()
                .format("%Y%m%d_%H%M%S")
        );

    export_vouchers_to_excel(
        &filename,
        &results,
        decimals,
    )
    .map_err(|e| {
        format!(
            "Failed to export Excel: {}",
            e
        )
    })?;

    println!();
    println!(
        "=========================================="
    );

    println!(
        "Voucher creation completed."
    );

    println!(
        "Mint             : {}",
        mint_pubkey
    );

    println!(
        "Decimals         : {}",
        decimals
    );

    println!(
        "Quota each       : {}",
        quota
    );

    println!(
        "Raw quota each   : {}",
        quota_amount
    );

    println!(
        "Count            : {}",
        results.len()
    );

    println!(
        "Vault            : {}",
        vault_pubkey
    );

    println!(
        "Vault ATA        : {}",
        vault_token_account
    );

    println!("Excel            : {}", filename);
    println!("!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
    println!("⚠️ IMPORTANT: Critical voucher data. Please save this file first!");
    println!("!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");

    println!(
        "=========================================="
    );    
    Ok(())
}

async fn command_redeem(
    config: &Config<'_>,
    signers: Vec<Arc<dyn Signer>>,
    code: String,
    payer_pbk: &Pubkey,
    program_id: &Pubkey,
) -> Result<(), Error> {
    let rpc_client = &config.rpc_client;
    let token2022_program_pbk = Pubkey::from_str("Token9ADbPtdFC3PjxaohBLGw2pgZwofdcbj6Lyaw6c").unwrap();
    let voucher_keypair = keypair_from_base58(&code)?;
    let voucher_pubkey = voucher_keypair.pubkey();
    println!("==========================================");
    println!("Voucher Redeem");
    println!("==========================================");
    let (treasury_pubkey, _treasury_bump) = find_pda(&[b"treasury_config"], program_id);

    let treasury_account = rpc_client.get_account(&treasury_pubkey).await
        .map_err(|_| format!("Treasury config account does not exist: {}", treasury_pubkey))?;
    let treasury_data = &treasury_account.data;
    if treasury_data.len() < 40 {
        return Err(format!("Treasury account data too short").into());
    }    
    let json_data = &treasury_data[40..];    
    if json_data.len() < 4 {
        return Err(format!("Invalid treasury JSON data: too short").into());
    }    
    let json_len = u32::from_le_bytes([json_data[0], json_data[1], json_data[2], json_data[3]]) as usize;    
    if json_data.len() < 4 + json_len {
        return Err(format!("Invalid treasury JSON length").into());
    }    
    let json_str = std::str::from_utf8(&json_data[4..4 + json_len])
        .map_err(|_| format!("Invalid UTF-8 in treasury config_json"))?;
    let treasury_config: TreasuryConfig = serde_json::from_str(json_str)
        .map_err(|e| format!("Failed to parse treasury config: {}", e))?;
    let fee_receiver_pubkey = Pubkey::from_str(&treasury_config.fee_receiver)
        .map_err(|_| format!("Invalid fee_receiver in treasury config"))?;
    let (detail_pubkey, detail_bump) =
        Pubkey::find_program_address(
            &[b"vo", voucher_pubkey.as_ref()],
            program_id,
        );
    let detail_account = rpc_client.get_account(&detail_pubkey)
            .await
            .map_err(|e| {
                format!(
                    "Voucher does not exist. \
                     Detail {} not found: {}",
                    detail_pubkey,
                    e
                )
            })?;

    let detail = parse_voucher_detail(&detail_account.data)?;
    let (vault_pubkey, vault_bump) =
        Pubkey::find_program_address(
            &[
                b"vault",
                detail.creator.as_ref(),
                detail.mint.as_ref(),
            ],
            program_id,
        );
    let user_token_account = get_or_create_token_ata(
            rpc_client,
            payer_pbk,
            &signers,
            payer_pbk,
            &detail.mint,
        )
        .await
        .map_err(|e| {
            format!(
                "Failed to get/create user ATA: {}",
                e
            )
        })?;

    //println!("Vault PDA: {} bump: {}", vault_pubkey, vault_bump);
    let vault_token_account = get_or_create_token_ata(
            rpc_client,
            payer_pbk,
            &signers,
            &vault_pubkey,
            &detail.mint,
        )
        .await
        .map_err(|e| {
            format!("Failed to get/create vault ATA: {}", e)
        })?;


    let accounts = vec![
        AccountMeta::new(*payer_pbk, true), // operator
        AccountMeta::new_readonly(voucher_pubkey, true),
        AccountMeta::new_readonly(detail.mint, false),
        AccountMeta::new(vault_pubkey, false),
        AccountMeta::new(detail_pubkey, false),
        AccountMeta::new(vault_token_account, false),
        AccountMeta::new(user_token_account, false),
        AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        AccountMeta::new_readonly(token2022_program_pbk, false),
    ];

    let mut data = Vec::new();
    data.extend_from_slice(&instruction_discriminator("voucher_redeem"));

    let instruction = Instruction {
        program_id: *program_id,
        accounts,
        data,
    };

    let blockhash = rpc_client.get_latest_blockhash().await?;
    let mut transaction = Transaction::new_with_payer(&[instruction], Some(payer_pbk));

    let mut all_signers: Vec<&dyn Signer> = Vec::new();

    for signer in signers.iter(){
        all_signers.push(
            signer.as_ref()
        );
    }
    all_signers.push(
        &voucher_keypair
    );
    transaction.sign(&all_signers, blockhash);

    let simulation = rpc_client.simulate_transaction(&transaction).await?;
    if let Some(err) = simulation.value.err {
        for log in simulation.value.logs.unwrap_or_default() {
            println!("{}", log);
        }
        return Err(format!("Simulation failed: {:?}", err).into());
    }
         
    let signature = rpc_client
        .send_and_confirm_transaction(&transaction)
        .await
        .map_err(|e| format!("Send failed: {}", e))?;
         
    println!("Voucher redeemed successfully!");

    println!("Public-key       : {}", voucher_pubkey);
    println!("User             : {}", payer_pbk);
    println!("Mint             : {}", detail.mint);
    println!("Raw Quota        : {}", detail.quota);
    println!("Vault            : {}", vault_pubkey);
    println!("Vault ATA        : {}", vault_token_account);
    println!("User ATA         : {}", user_token_account);
    println!("Transaction      : {}", signature);
    println!("==========================================");
    Ok(())
}

async fn command_transfer_ownership(
    config: &Config<'_>,
    signers: Vec<Arc<dyn Signer>>,
    name: String,
    to: Option<Pubkey>,
    payer_pbk: &Pubkey,
    program_id: &Pubkey,
) -> Result<(), Error> {
    let rpc_client = &config.rpc_client;    
    let name_len = name.len();
    if name_len == 0 || name_len > 100 {
        return Err(format!("Name length must be 1-100").into());
    }
    println!("ecosystem ownership transfer...");
    let to_pbk = to.ok_or_else(|| "Error: '--to' is required but was not provided")?;
    let name_hash = hash_name(&name);
    let (header_pubkey, _header_bump) = find_pda(&[b"name", &name_hash], program_id);
    let (meta_pubkey, _meta_bump) = find_pda(&[b"meta", &name_hash], program_id);
    let (body_pubkey, _body_bump) = find_pda(&[b"body", &name_hash], program_id);

    let accounts = vec![
        AccountMeta::new(header_pubkey, false),
        AccountMeta::new(meta_pubkey, false),
        AccountMeta::new(body_pubkey, false),
        AccountMeta::new(*payer_pbk, true), // operator
        AccountMeta::new(to_pbk, false), // to
        AccountMeta::new_readonly(solana_sdk::system_program::ID, false)
    ];

    let mut data = Vec::new();
    data.extend_from_slice(&instruction_discriminator("eco_owner_transfer"));

    data.extend_from_slice(&(name.len() as u32).to_le_bytes());
    data.extend_from_slice(name.as_bytes());

    data.extend_from_slice(&to_pbk.to_bytes());

    let instruction = Instruction {
        program_id: *program_id,
        accounts,
        data,
    };

    let blockhash = rpc_client.get_latest_blockhash().await?;
    let mut transaction = Transaction::new_with_payer(&[instruction], Some(payer_pbk));
    transaction.sign(&signers, blockhash);

    let simulation = rpc_client.simulate_transaction(&transaction).await?;
    if let Some(err) = simulation.value.err {
        for log in simulation.value.logs.unwrap_or_default() {
            println!("{}", log);
        }
        return Err(format!("Simulation failed: {:?}", err).into());
    }
         
    let signature = rpc_client
        .send_and_confirm_transaction(&transaction)
        .await
        .map_err(|e| format!("Send failed: {}", e))?;
         
    println!("ecosystem ownership transfer finished. hash: {}", signature);
    Ok(())
}

async fn command_sale_ask(
    config: &Config<'_>,
    signers: Vec<Arc<dyn Signer>>,
    name: String,
    enabled: u8,
    sell_price: u64,
    payer_pbk: &Pubkey,
    program_id: &Pubkey,
) -> Result<(), Error> {
    let rpc_client = &config.rpc_client;
    println!("ecosystem sale ask...");
    let name_hash = hash_name(&name);
    let (header_pubkey, _header_bump) = find_pda(&[b"name", &name_hash], program_id);

    let accounts = vec![
        AccountMeta::new(header_pubkey, false),
        AccountMeta::new(*payer_pbk, true), // operator
        AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ];

    let mut data = Vec::new();
    data.extend_from_slice(&instruction_discriminator("eco_sale_ask"));

    data.extend_from_slice(&(name.len() as u32).to_le_bytes());
    data.extend_from_slice(name.as_bytes());
    data.push(enabled);
    data.extend_from_slice(&sell_price.to_le_bytes()); //u64

    let instruction = Instruction {
        program_id: *program_id,
        accounts,
        data,
    };

    let blockhash = rpc_client.get_latest_blockhash().await?;
    let mut transaction = Transaction::new_with_payer(&[instruction], Some(payer_pbk));
    transaction.sign(&signers, blockhash);

    let simulation = rpc_client.simulate_transaction(&transaction).await?;
    if let Some(err) = simulation.value.err {
        for log in simulation.value.logs.unwrap_or_default() {
            println!("{}", log);
        }
        return Err(format!("Simulation failed: {:?}", err).into());
    }
         
    let signature = rpc_client
        .send_and_confirm_transaction(&transaction)
        .await
        .map_err(|e| format!("Send failed: {}", e))?;
         
    println!("ecosystem sale ask finished. hash: {}", signature);
    Ok(())
}

async fn command_sale_bid(
    config: &Config<'_>,
    signers: Vec<Arc<dyn Signer>>,
    name: String,
    payer_pbk: &Pubkey,
    program_id: &Pubkey,
) -> Result<(), Error> {
    let rpc_client = &config.rpc_client;    
    let name_len = name.len();
    if name_len == 0 || name_len > 100 {
        return Err(format!("Name length must be 1-100").into());
    }
    println!("ecosystem sale bid...");
    let name_hash = hash_name(&name);
    let (header_pubkey, _header_bump) = find_pda(&[b"name", &name_hash], program_id);
    let (meta_pubkey, _meta_bump) = find_pda(&[b"meta", &name_hash], program_id);
    let (body_pubkey, _body_bump) = find_pda(&[b"body", &name_hash], program_id);

    let meta_account = rpc_client.get_account(&meta_pubkey).await
        .map_err(|_| format!("Meta config account does not exist: {}", meta_pubkey))?;
    let meta_data = &meta_account.data;
    if meta_data.len() < 40 {
        return Err(format!("Meta account data too short").into());
    }    
    let json_data = &meta_data[40..];    
    if json_data.len() < 4 {
        return Err(format!("Invalid meta JSON data: too short").into());
    }    
    let json_len = u32::from_le_bytes([json_data[0], json_data[1], json_data[2], json_data[3]]) as usize;    
    if json_data.len() < 4 + json_len {
        return Err(format!("Invalid JSON length").into());
    }    
    let json_str = std::str::from_utf8(&json_data[4..4 + json_len])
        .map_err(|_| format!("Invalid UTF-8 in meta config_json"))?;
    let meta_config: MetaConfig = serde_json::from_str(json_str)
        .map_err(|e| format!("Failed to parse meta config: {}", e))?;
    let fee_receiver_pubkey = Pubkey::from_str(&meta_config.fee_receiver)
        .map_err(|_| format!("Invalid fee_receiver in meta config"))?;
    let pay_token_str = &meta_config.pay_token.unwrap();    
    let token2022_program_pbk = Pubkey::from_str("Token9ADbPtdFC3PjxaohBLGw2pgZwofdcbj6Lyaw6c").unwrap();

    let mut accounts = vec![
        AccountMeta::new(header_pubkey, false),
        AccountMeta::new(meta_pubkey, false),
        AccountMeta::new(body_pubkey, false),
        AccountMeta::new(*payer_pbk, true), // operator
        AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        AccountMeta::new(fee_receiver_pubkey, false), //fee_receiver
        AccountMeta::new_readonly(token2022_program_pbk, false),
        AccountMeta::new_readonly(spl_associated_token_account::ID, false),
    ];
    if pay_token_str != "11111111111111111111111111111111111111111111" {
        let mint_pubkey = Pubkey::from_str(pay_token_str)?;
        let from_ata = spl_associated_token_account::get_associated_token_address_with_program_id(
            payer_pbk, 
            &mint_pubkey,
            &token2022_program_pbk
        );
        let to_ata = spl_associated_token_account::get_associated_token_address_with_program_id(
            &fee_receiver_pubkey,
            &mint_pubkey, 
            &token2022_program_pbk   
        );
        accounts.push(AccountMeta::new(from_ata, false));
        accounts.push(AccountMeta::new(to_ata, false));
        accounts.push(AccountMeta::new(mint_pubkey, false));
    }else{
        let mint_pubkey = Pubkey::from_str("Cmdnkd1MJBfKuBjp3j33BqeesZCYrnJ4mXnk19Uhs3z2")?;
        let ata = Pubkey::from_str("6gymrEA7R98Y1dffrTng5BCstmxjgdN692DxRsFtoMvM")?;
        accounts.push(AccountMeta::new(ata, false));
        accounts.push(AccountMeta::new(ata, false));
        accounts.push(AccountMeta::new(mint_pubkey, false));
    } 

    let mut data = Vec::new();
    data.extend_from_slice(&instruction_discriminator("eco_sale_bid"));

    data.extend_from_slice(&(name.len() as u32).to_le_bytes());
    data.extend_from_slice(name.as_bytes());

    let instruction = Instruction {
        program_id: *program_id,
        accounts,
        data,
    };

    let blockhash = rpc_client.get_latest_blockhash().await?;
    let mut transaction = Transaction::new_with_payer(&[instruction], Some(payer_pbk));
    transaction.sign(&signers, blockhash);

    let simulation = rpc_client.simulate_transaction(&transaction).await?;
    if let Some(err) = simulation.value.err {
        for log in simulation.value.logs.unwrap_or_default() {
            println!("{}", log);
        }
        return Err(format!("Simulation failed: {:?}", err).into());
    }
         
    let signature = rpc_client
        .send_and_confirm_transaction(&transaction)
        .await
        .map_err(|e| format!("Send failed: {}", e))?;
         
    println!("ecosystem sale bid finished. hash: {}", signature);
    Ok(())
}

async fn command_rent_ask(
    config: &Config<'_>,
    signers: Vec<Arc<dyn Signer>>,
    name: String,
    enabled: u8,
    rent_per_day: u64,
    payer_pbk: &Pubkey,
    program_id: &Pubkey,
) -> Result<(), Error> {
    let rpc_client = &config.rpc_client;
    println!("ecosystem rent ask...");
    let name_hash = hash_name(&name);
    let (header_pubkey, _header_bump) = find_pda(&[b"name", &name_hash], program_id);

    let accounts = vec![
        AccountMeta::new(header_pubkey, false),
        AccountMeta::new(*payer_pbk, true), // operator
        AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ];

    let mut data = Vec::new();
    data.extend_from_slice(&instruction_discriminator("eco_rent_ask"));

    data.extend_from_slice(&(name.len() as u32).to_le_bytes());
    data.extend_from_slice(name.as_bytes());
    data.push(enabled);
    data.extend_from_slice(&rent_per_day.to_le_bytes()); //u64

    let instruction = Instruction {
        program_id: *program_id,
        accounts,
        data,
    };

    let blockhash = rpc_client.get_latest_blockhash().await?;
    let mut transaction = Transaction::new_with_payer(&[instruction], Some(payer_pbk));
    transaction.sign(&signers, blockhash);

    let simulation = rpc_client.simulate_transaction(&transaction).await?;
    if let Some(err) = simulation.value.err {
        for log in simulation.value.logs.unwrap_or_default() {
            println!("{}", log);
        }
        return Err(format!("Simulation failed: {:?}", err).into());
    }
         
    let signature = rpc_client
        .send_and_confirm_transaction(&transaction)
        .await
        .map_err(|e| format!("Send failed: {}", e))?;
         
    println!("ecosystem rent ask finished. hash: {}", signature);
    Ok(())
}

async fn command_rent_bid(
    config: &Config<'_>,
    signers: Vec<Arc<dyn Signer>>,
    name: String,
    days: u32,
    payer_pbk: &Pubkey,
    program_id: &Pubkey,
) -> Result<(), Error> {
    let rpc_client = &config.rpc_client;    
    let name_len = name.len();
    if name_len == 0 || name_len > 100 {
        return Err(format!("Name length must be 1-100").into());
    }
    println!("ecosystem rent bid...");
    let name_hash = hash_name(&name);
    let (header_pubkey, _header_bump) = find_pda(&[b"name", &name_hash], program_id);
    let (meta_pubkey, _meta_bump) = find_pda(&[b"meta", &name_hash], program_id);
    let (rent_pubkey, _rent_bump) = find_pda(&[b"rent", &name_hash], program_id);

    let meta_account = rpc_client.get_account(&meta_pubkey).await
        .map_err(|_| format!("Meta config account does not exist: {}", meta_pubkey))?;
    let meta_data = &meta_account.data;
    if meta_data.len() < 40 {
        return Err(format!("Meta account data too short").into());
    }    
    let json_data = &meta_data[40..];    
    if json_data.len() < 4 {
        return Err(format!("Invalid meta JSON data: too short").into());
    }    
    let json_len = u32::from_le_bytes([json_data[0], json_data[1], json_data[2], json_data[3]]) as usize;    
    if json_data.len() < 4 + json_len {
        return Err(format!("Invalid JSON length").into());
    }    
    let json_str = std::str::from_utf8(&json_data[4..4 + json_len])
        .map_err(|_| format!("Invalid UTF-8 in meta config_json"))?;
    let meta_config: MetaConfig = serde_json::from_str(json_str)
        .map_err(|e| format!("Failed to parse meta config: {}", e))?;
    let fee_receiver_pubkey = Pubkey::from_str(&meta_config.fee_receiver)
        .map_err(|_| format!("Invalid fee_receiver in meta config"))?;
    let pay_token_str = &meta_config.pay_token.unwrap();    
    let token2022_program_pbk = Pubkey::from_str("Token9ADbPtdFC3PjxaohBLGw2pgZwofdcbj6Lyaw6c").unwrap();
    
    let mut accounts = vec![
        AccountMeta::new(header_pubkey, false),
        AccountMeta::new(meta_pubkey, false),
        AccountMeta::new(rent_pubkey, false),
        AccountMeta::new(*payer_pbk, true), // operator
        AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        AccountMeta::new(fee_receiver_pubkey, false), //fee_receiver
    ];

    if pay_token_str != "11111111111111111111111111111111111111111111" {
        let mint_pubkey = Pubkey::from_str(pay_token_str)?;
        let from_ata = spl_associated_token_account::get_associated_token_address_with_program_id(
            payer_pbk, 
            &mint_pubkey,
            &token2022_program_pbk
        );
        let to_ata = spl_associated_token_account::get_associated_token_address_with_program_id(
            &fee_receiver_pubkey,
            &mint_pubkey, 
            &token2022_program_pbk   
        );
        accounts.push(AccountMeta::new(from_ata, false));
        accounts.push(AccountMeta::new(to_ata, false));
        accounts.push(AccountMeta::new(mint_pubkey, false));
    }else{
        let mint_pubkey = Pubkey::from_str("Cmdnkd1MJBfKuBjp3j33BqeesZCYrnJ4mXnk19Uhs3z2")?;
        let ata = Pubkey::from_str("6gymrEA7R98Y1dffrTng5BCstmxjgdN692DxRsFtoMvM")?;
        accounts.push(AccountMeta::new(ata, false));
        accounts.push(AccountMeta::new(ata, false));
        accounts.push(AccountMeta::new(mint_pubkey, false));
    } 
    accounts.push(AccountMeta::new_readonly(token2022_program_pbk, false));
    accounts.push(AccountMeta::new_readonly(spl_associated_token_account::ID, false));

    let mut data = Vec::new();
    data.extend_from_slice(&instruction_discriminator("eco_rent_bid"));

    data.extend_from_slice(&(name.len() as u32).to_le_bytes());
    data.extend_from_slice(name.as_bytes());

    data.extend_from_slice(&days.to_le_bytes()); //u32

    let instruction = Instruction {
        program_id: *program_id,
        accounts,
        data,
    };

    let blockhash = rpc_client.get_latest_blockhash().await?;
    let mut transaction = Transaction::new_with_payer(&[instruction], Some(payer_pbk));
    transaction.sign(&signers, blockhash);

    let simulation = rpc_client.simulate_transaction(&transaction).await?;
    if let Some(err) = simulation.value.err {
        for log in simulation.value.logs.unwrap_or_default() {
            println!("{}", log);
        }
        return Err(format!("Simulation failed: {:?}", err).into());
    }
         
    let signature = rpc_client
        .send_and_confirm_transaction(&transaction)
        .await
        .map_err(|e| format!("Send failed: {}", e))?;
         
    println!("ecosystem rent bid finished. hash: {}", signature);
    Ok(())
}

async fn command_update_rent(
    config: &Config<'_>,
    signers: Vec<Arc<dyn Signer>>,
    name: String,
    rent_info: String,
    payer_pbk: &Pubkey,
    program_id: &Pubkey,
) -> Result<(), Error> {
    let rpc_client = &config.rpc_client;
    println!("ecosystem update rent...");
    let name_hash = hash_name(&name);
    let (header_pubkey, _header_bump) = find_pda(&[b"name", &name_hash], program_id);
    let (rent_pubkey, _rent_bump) = find_pda(&[b"rent", &name_hash], program_id);

    let accounts = vec![
        AccountMeta::new(header_pubkey, false),
        AccountMeta::new(rent_pubkey, false),
        AccountMeta::new(*payer_pbk, true), // operator
        AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ];

    let mut data = Vec::new();
    data.extend_from_slice(&instruction_discriminator("eco_rent_info_update"));

    data.extend_from_slice(&(name.len() as u32).to_le_bytes());
    data.extend_from_slice(name.as_bytes());

    data.extend_from_slice(&(rent_info.len() as u32).to_le_bytes());
    data.extend_from_slice(rent_info.as_bytes());

    let instruction = Instruction {
        program_id: *program_id,
        accounts,
        data,
    };

    let blockhash = rpc_client.get_latest_blockhash().await?;
    let mut transaction = Transaction::new_with_payer(&[instruction], Some(payer_pbk));
    transaction.sign(&signers, blockhash);

    let simulation = rpc_client.simulate_transaction(&transaction).await?;
    if let Some(err) = simulation.value.err {
        for log in simulation.value.logs.unwrap_or_default() {
            println!("{}", log);
        }
        return Err(format!("Simulation failed: {:?}", err).into());
    }
         
    let signature = rpc_client
        .send_and_confirm_transaction(&transaction)
        .await
        .map_err(|e| format!("Send failed: {}", e))?;
         
    println!("ecosystem update rent finished. hash: {}", signature);
    Ok(())
}

async fn command_renew_usership(
    config: &Config<'_>,
    signers: Vec<Arc<dyn Signer>>,
    name: String,
    days: u32,
    payer_pbk: &Pubkey,
    program_id: &Pubkey,
) -> Result<(), Error> {
    let rpc_client = &config.rpc_client;    
    let name_len = name.len();
    if name_len == 0 || name_len > 100 {
        return Err(format!("Name length must be 1-100").into());
    }
    if days < 1 {
        return Err(format!("Days must be >= 1").into());
    }
    println!("ecosystem usership renew...");
    let name_hash = hash_name(&name);
    let (header_pubkey, _header_bump) = find_pda(&[b"name", &name_hash], program_id);
    let (meta_pubkey, _meta_bump) = find_pda(&[b"meta", &name_hash], program_id);

    let meta_account = rpc_client.get_account(&meta_pubkey).await
        .map_err(|_| format!("Meta config account does not exist: {}", meta_pubkey))?;
    let meta_data = &meta_account.data;
    if meta_data.len() < 40 {
        return Err(format!("Meta account data too short").into());
    }    
    let json_data = &meta_data[40..];    
    if json_data.len() < 4 {
        return Err(format!("Invalid meta JSON data: too short").into());
    }    
    let json_len = u32::from_le_bytes([json_data[0], json_data[1], json_data[2], json_data[3]]) as usize;    
    if json_data.len() < 4 + json_len {
        return Err(format!("Invalid JSON length").into());
    }    
    let json_str = std::str::from_utf8(&json_data[4..4 + json_len])
        .map_err(|_| format!("Invalid UTF-8 in meta config_json"))?;
    let meta_config: MetaConfig = serde_json::from_str(json_str)
        .map_err(|e| format!("Failed to parse meta config: {}", e))?;
    let fee_receiver_pubkey = Pubkey::from_str(&meta_config.fee_receiver)
        .map_err(|_| format!("Invalid fee_receiver in meta config"))?;
    let pay_token_str = &meta_config.pay_token.unwrap();    
    let token2022_program_pbk = Pubkey::from_str("Token9ADbPtdFC3PjxaohBLGw2pgZwofdcbj6Lyaw6c").unwrap();

    let mut accounts = vec![
        AccountMeta::new(header_pubkey, false),
        AccountMeta::new(*payer_pbk, true), // operator
        AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        AccountMeta::new(meta_pubkey, false),
        AccountMeta::new(fee_receiver_pubkey, false), //fee_receiver
    ];
    if pay_token_str != "11111111111111111111111111111111111111111111" {
        let mint_pubkey = Pubkey::from_str(pay_token_str)?;
        let from_ata = spl_associated_token_account::get_associated_token_address_with_program_id(
            payer_pbk, 
            &mint_pubkey,
            &token2022_program_pbk
        );
        let to_ata = spl_associated_token_account::get_associated_token_address_with_program_id(
            &fee_receiver_pubkey,
            &mint_pubkey, 
            &token2022_program_pbk   
        );
        accounts.push(AccountMeta::new(from_ata, false));
        accounts.push(AccountMeta::new(to_ata, false));
        accounts.push(AccountMeta::new(mint_pubkey, false));
    }else{
        let mint_pubkey = Pubkey::from_str("Cmdnkd1MJBfKuBjp3j33BqeesZCYrnJ4mXnk19Uhs3z2")?;
        let ata = Pubkey::from_str("6gymrEA7R98Y1dffrTng5BCstmxjgdN692DxRsFtoMvM")?;
        accounts.push(AccountMeta::new(ata, false));
        accounts.push(AccountMeta::new(ata, false));
        accounts.push(AccountMeta::new(mint_pubkey, false));
    } 
    accounts.push(AccountMeta::new_readonly(token2022_program_pbk, false));
    accounts.push(AccountMeta::new_readonly(spl_associated_token_account::ID, false));

    let mut data = Vec::new();
    data.extend_from_slice(&instruction_discriminator("eco_rent_renew"));

    data.extend_from_slice(&(name.len() as u32).to_le_bytes());
    data.extend_from_slice(name.as_bytes());

    data.extend_from_slice(&days.to_le_bytes()); //u32

    let instruction = Instruction {
        program_id: *program_id,
        accounts,
        data,
    };

    let blockhash = rpc_client.get_latest_blockhash().await?;
    let mut transaction = Transaction::new_with_payer(&[instruction], Some(payer_pbk));
    transaction.sign(&signers, blockhash);

    let simulation = rpc_client.simulate_transaction(&transaction).await?;
    if let Some(err) = simulation.value.err {
        for log in simulation.value.logs.unwrap_or_default() {
            println!("{}", log);
        }
        return Err(format!("Simulation failed: {:?}", err).into());
    }
         
    let signature = rpc_client
        .send_and_confirm_transaction(&transaction)
        .await
        .map_err(|e| format!("Send failed: {}", e))?;
         
    println!("ecosystem usership renew finished. hash: {}", signature);
    Ok(())
}

async fn command_transfer_usership(
    config: &Config<'_>,
    signers: Vec<Arc<dyn Signer>>,
    name: String,
    to: Option<Pubkey>,
    payer_pbk: &Pubkey,
    program_id: &Pubkey,
) -> Result<(), Error> {
    let rpc_client = &config.rpc_client;    
    let name_len = name.len();
    if name_len == 0 || name_len > 100 {
        return Err(format!("Name length must be 1-100").into());
    }
    println!("ecosystem usership transfer...");
    let to_pbk = to.ok_or_else(|| "Error: '--to' is required but was not provided")?;
    let name_hash = hash_name(&name);
    let (header_pubkey, _header_bump) = find_pda(&[b"name", &name_hash], program_id);
    let (rent_pubkey, _rent_bump) = find_pda(&[b"rent", &name_hash], program_id);

    let accounts = vec![
        AccountMeta::new(header_pubkey, false),
        AccountMeta::new(rent_pubkey, false),
        AccountMeta::new(*payer_pbk, true), // operator
        AccountMeta::new(to_pbk, false), // to
        AccountMeta::new_readonly(solana_sdk::system_program::ID, false)
    ];

    let mut data = Vec::new();
    data.extend_from_slice(&instruction_discriminator("eco_rent_transfer"));

    data.extend_from_slice(&(name.len() as u32).to_le_bytes());
    data.extend_from_slice(name.as_bytes());

    data.extend_from_slice(&to_pbk.to_bytes());

    let instruction = Instruction {
        program_id: *program_id,
        accounts,
        data,
    };

    let blockhash = rpc_client.get_latest_blockhash().await?;
    let mut transaction = Transaction::new_with_payer(&[instruction], Some(payer_pbk));
    transaction.sign(&signers, blockhash);

    let simulation = rpc_client.simulate_transaction(&transaction).await?;
    if let Some(err) = simulation.value.err {
        for log in simulation.value.logs.unwrap_or_default() {
            println!("{}", log);
        }
        return Err(format!("Simulation failed: {:?}", err).into());
    }
         
    let signature = rpc_client
        .send_and_confirm_transaction(&transaction)
        .await
        .map_err(|e| format!("Send failed: {}", e))?;
         
    println!("ecosystem usership transfer finished. hash: {}", signature);
    Ok(())
}
