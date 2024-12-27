 use anchor_lang::{
    prelude::*,
    solana_program::{
        instruction::Instruction,
        program_error::ProgramError,
        program_pack::Pack,
        system_instruction, system_program,
        sysvar::{self},
    },
};
use solana_program_test::*;
use solana_sdk::{
    entrypoint::ProgramResult,
    signature::{Keypair, Signer},
    transaction::Transaction,
    transport::TransportError,
};

// Custom error type for our tests
#[derive(Debug)]
enum TestError {
    BanksClientError(BanksClientError),
    ProgramError(ProgramError),
    TransportError(TransportError),
    PackError(String),
}

impl From<BanksClientError> for TestError {
    fn from(e: BanksClientError) -> Self {
        Self::BanksClientError(e)
    }
}

impl From<ProgramError> for TestError {
    fn from(e: ProgramError) -> Self {
        Self::ProgramError(e)
    }
}

impl From<TransportError> for TestError {
    fn from(e: TransportError) -> Self {
        Self::TransportError(e)
    }
}

type TestResult<T> = Result<T, TestError>;

// Helper function for token account unpacking errors
fn handle_token_error<T: std::fmt::Display>(e: T) -> TestError {
    TestError::PackError(e.to_string())
}

async fn setup_mint(
    banks_client: &mut BanksClient,
    payer: &Keypair,
    mint_keypair: &Keypair,
    mint_authority: &Keypair,
) -> TestResult<()> {
    let rent = banks_client
        .get_rent()
        .await
        .map_err(TestError::BanksClientError)?;
    let mint_rent = rent.minimum_balance(spl_token::state::Mint::LEN);

    let create_mint_account_ix = system_instruction::create_account(
        &payer.pubkey(),
        &mint_keypair.pubkey(),
        mint_rent,
        spl_token::state::Mint::LEN as u64,
        &spl_token::id(),
    );

    let initialize_mint_ix = spl_token::instruction::initialize_mint(
        &spl_token::id(),
        &mint_keypair.pubkey(),
        &mint_authority.pubkey(),
        None,
        9,
    )?;

    let recent_blockhash = banks_client.get_latest_blockhash().await?;
    let transaction = Transaction::new_signed_with_payer(
        &[create_mint_account_ix, initialize_mint_ix],
        Some(&payer.pubkey()),
        &[payer, mint_keypair],
        recent_blockhash,
    );

    banks_client.process_transaction(transaction).await?;
    Ok(())
}

async fn create_token_account(
    banks_client: &mut BanksClient,
    payer: &Keypair,
    mint: &Pubkey,
    owner: &Pubkey,
) -> TestResult<Pubkey> {
    let token_account = Keypair::new();
    let rent = banks_client.get_rent().await?;
    let account_rent = rent.minimum_balance(spl_token::state::Account::LEN);

    let create_account_ix = system_instruction::create_account(
        &payer.pubkey(),
        &token_account.pubkey(),
        account_rent,
        spl_token::state::Account::LEN as u64,
        &spl_token::id(),
    );

    let initialize_account_ix = spl_token::instruction::initialize_account(
        &spl_token::id(),
        &token_account.pubkey(),
        mint,
        owner,
    )?;

    let recent_blockhash = banks_client.get_latest_blockhash().await?;
    let transaction = Transaction::new_signed_with_payer(
        &[create_account_ix, initialize_account_ix],
        Some(&payer.pubkey()),
        &[payer, &token_account],
        recent_blockhash,
    );

    banks_client.process_transaction(transaction).await?;
    Ok(token_account.pubkey())
}

async fn mint_tokens(
    banks_client: &mut BanksClient,
    payer: &Keypair,
    mint: &Pubkey,
    token_account: &Pubkey,
    mint_authority: &Keypair,
    amount: u64,
) -> TestResult<()> {
    let mint_ix = spl_token::instruction::mint_to(
        &spl_token::id(),
        mint,
        token_account,
        &mint_authority.pubkey(),
        &[],
        amount,
    )?;

    let recent_blockhash = banks_client.get_latest_blockhash().await?;
    let transaction = Transaction::new_signed_with_payer(
        &[mint_ix],
        Some(&payer.pubkey()),
        &[payer, mint_authority],
        recent_blockhash,
    );

    banks_client.process_transaction(transaction).await?;
    Ok(())
}

#[tokio::test]
async fn test_full_flow() -> TestResult<()> {
    let program_id = abc_token::id();
    let raydium_program_id = Pubkey::new_unique();

    let mut program_test = ProgramTest::new("abc_token", program_id, processor!(abc_token::entry));

    // Add mock Raydium program
    program_test.add_program(
        "raydium",
        raydium_program_id,
        processor!(mock_raydium_processor),
    );

    // Start the test context
    let (mut banks_client, payer, recent_blockhash) = program_test.start().await;

    // Setup mint
    let mint_keypair = Keypair::new();
    let mint_authority = Keypair::new();
    setup_mint(&mut banks_client, &payer, &mint_keypair, &mint_authority).await?;

    // Setup token accounts
    let token_source = create_token_account(
        &mut banks_client,
        &payer,
        &mint_keypair.pubkey(),
        &mint_authority.pubkey(),
    )
    .await?;

    // Mint initial supply
    mint_tokens(
        &mut banks_client,
        &payer,
        &mint_keypair.pubkey(),
        &token_source,
        &mint_authority,
        1_000_000_000_000,
    )
    .await?;

    // Setup Raydium accounts
    let (pool_account, token_vault, sol_vault) =
        setup_raydium_accounts(&mut banks_client, &payer, &mint_keypair.pubkey()).await?;

    // Initialize ABC token program
    let (manager, reserve_account) = initialize_abc_token(
        &mut banks_client,
        &payer,
        &mint_authority,
        &mint_keypair.pubkey(),
        &token_source,
        &pool_account,
        &token_vault,
        &sol_vault,
        &raydium_program_id,
    )
    .await?;

    // Test bot detection
    test_bot_detection(
        &mut banks_client,
        &payer,
        &manager,
        &pool_account,
        &token_vault,
        &sol_vault,
        &mint_keypair.pubkey(),
        &raydium_program_id,
    )
    .await?;

    // Test normal trading
    test_normal_trading(
        &mut banks_client,
        &payer,
        &manager,
        &pool_account,
        &token_vault,
        &sol_vault,
        &mint_keypair.pubkey(),
        &raydium_program_id,
    )
    .await?;

    Ok(())
}

async fn setup_raydium_accounts(
    banks_client: &mut BanksClient,
    payer: &Keypair,
    mint: &Pubkey,
) -> TestResult<(Pubkey, Pubkey, Pubkey)> {
    let pool_account = Keypair::new();
    let token_vault =
        create_token_account(banks_client, payer, mint, &pool_account.pubkey()).await?;

    let sol_vault = Keypair::new();
    let rent = banks_client.get_rent().await?;
    let sol_vault_rent = rent.minimum_balance(0);

    let create_sol_vault_ix = system_instruction::create_account(
        &payer.pubkey(),
        &sol_vault.pubkey(),
        sol_vault_rent,
        0,
        &system_program::id(),
    );

    let recent_blockhash = banks_client.get_latest_blockhash().await?;
    let transaction = Transaction::new_signed_with_payer(
        &[create_sol_vault_ix],
        Some(&payer.pubkey()),
        &[payer, &sol_vault],
        recent_blockhash,
    );

    banks_client.process_transaction(transaction).await?;

    Ok((pool_account.pubkey(), token_vault, sol_vault.pubkey()))
}

async fn initialize_abc_token(
    banks_client: &mut BanksClient,
    payer: &Keypair,
    authority: &Keypair,
    mint: &Pubkey,
    token_source: &Pubkey,
    pool_account: &Pubkey,
    token_vault: &Pubkey,
    sol_vault: &Pubkey,
    raydium_program_id: &Pubkey,
) -> TestResult<(Pubkey, Pubkey)> {
    let (manager, _) =
        Pubkey::find_program_address(&[b"abc_manager", mint.as_ref()], &abc_token::id());

    let (reserve_account, _) =
        Pubkey::find_program_address(&[b"reserve", mint.as_ref()], &abc_token::id());

    let accounts = vec![
        AccountMeta::new(authority.pubkey(), true),
        AccountMeta::new_readonly(*mint, false),
        AccountMeta::new(manager, false),
        AccountMeta::new(*token_source, false),
        AccountMeta::new(reserve_account, false),
        AccountMeta::new(*pool_account, false),
        AccountMeta::new(*token_vault, false),
        AccountMeta::new(*sol_vault, false),
        AccountMeta::new_readonly(*raydium_program_id, false),
        AccountMeta::new_readonly(spl_token::id(), false),
        AccountMeta::new_readonly(system_program::id(), false),
        AccountMeta::new_readonly(sysvar::rent::id(), false),
        AccountMeta::new_readonly(sysvar::clock::id(), false),
    ];

    let mut init_data = vec![0; 8 + 8]; // 8 bytes discriminator + 8 bytes for u64
    init_data[0..8].copy_from_slice(&[103, 133, 90, 210, 225, 25, 126, 37]); // Initialize discriminator
    init_data[8..16].copy_from_slice(&1_000_000_000_000u64.to_le_bytes());

    let init_ix = Instruction {
        program_id: abc_token::id(),
        accounts,
        data: init_data,
    };

    let recent_blockhash = banks_client.get_latest_blockhash().await?;
    let transaction = Transaction::new_signed_with_payer(
        &[init_ix],
        Some(&payer.pubkey()),
        &[payer, authority],
        recent_blockhash,
    );

    banks_client.process_transaction(transaction).await?;

    // Verify initialization
    let manager_account = banks_client.get_account(manager).await?.unwrap();
    let manager_data = abc_token::ABCManager::try_deserialize(&mut &manager_account.data[..])?;

    assert_eq!(manager_data.authority, authority.pubkey());
    assert_eq!(manager_data.mint, *mint);
    assert!(manager_data.is_launched);

    Ok((manager, reserve_account))
}

async fn test_bot_detection(
    banks_client: &mut BanksClient,
    payer: &Keypair,
    manager: &Pubkey,
    pool_account: &Pubkey,
    token_vault: &Pubkey,
    sol_vault: &Pubkey,
    mint: &Pubkey,
    raydium_program_id: &Pubkey,
) -> TestResult<()> {
    let bot_trader = Keypair::new();
    let bot_token_account =
        create_token_account(banks_client, payer, mint, &bot_trader.pubkey()).await?;

    // Fund bot trader
    let recent_blockhash = banks_client.get_latest_blockhash().await?;
    let fund_tx = Transaction::new_signed_with_payer(
        &[system_instruction::transfer(
            &payer.pubkey(),
            &bot_trader.pubkey(),
            5_000_000_000, // 5 SOL
        )],
        Some(&payer.pubkey()),
        &[payer],
        recent_blockhash,
    );
    banks_client.process_transaction(fund_tx).await?;

    // Execute trade during monitoring period
    let accounts = vec![
        AccountMeta::new(*manager, false),
        AccountMeta::new(bot_trader.pubkey(), true),
        AccountMeta::new(bot_token_account, false),
        AccountMeta::new(*pool_account, false),
        AccountMeta::new(*token_vault, false),
        AccountMeta::new(*sol_vault, false),
        AccountMeta::new_readonly(*raydium_program_id, false),
        AccountMeta::new_readonly(spl_token::id(), false),
        AccountMeta::new_readonly(system_program::id(), false),
        AccountMeta::new_readonly(sysvar::clock::id(), false),
    ];

    let mut buy_data = vec![0; 8 + 8];
    buy_data[0..8].copy_from_slice(&[242, 35, 198, 137, 82, 225, 242, 182]); // Buy discriminator
    buy_data[8..16].copy_from_slice(&1_000_000_000u64.to_le_bytes());

    let buy_ix = Instruction {
        program_id: abc_token::id(),
        accounts,
        data: buy_data,
    };

    let recent_blockhash = banks_client.get_latest_blockhash().await?;
    let transaction = Transaction::new_signed_with_payer(
        &[buy_ix],
        Some(&payer.pubkey()),
        &[payer, &bot_trader],
        recent_blockhash,
    );

    banks_client.process_transaction(transaction).await?;

    // Verify bot detection
    let manager_account = banks_client.get_account(*manager).await?.unwrap();
    let manager_data = abc_token::ABCManager::try_deserialize(&mut &manager_account.data[..])?;

    assert_eq!(manager_data.last_blocked_address, bot_trader.pubkey());
    assert_eq!(manager_data.captured_sol, 1_000_000_000);

    // Check token balances to verify counter-trade
    let bot_token_balance = get_token_balance(banks_client, &bot_token_account).await?;
    let pool_token_balance = get_token_balance(banks_client, token_vault).await?;

    // Bot should have received tokens, but counter-trade should have neutralized price impact
    assert!(bot_token_balance > 0);
    assert!(pool_token_balance > 0);

    Ok(())
}

async fn test_normal_trading(
    banks_client: &mut BanksClient,
    payer: &Keypair,
    manager: &Pubkey,
    pool_account: &Pubkey,
    token_vault: &Pubkey,
    sol_vault: &Pubkey,
    mint: &Pubkey,
    raydium_program_id: &Pubkey,
) -> TestResult<()> {
    // Advance clock past monitoring period by processing empty transactions
    for _ in 0..6 {
        let recent_blockhash = banks_client.get_latest_blockhash().await?;
        let tx = Transaction::new_signed_with_payer(
            &[],
            Some(&payer.pubkey()),
            &[payer],
            recent_blockhash,
        );
        banks_client.process_transaction(tx).await?;
    }

    let normal_trader = Keypair::new();
    let normal_token_account =
        create_token_account(banks_client, payer, mint, &normal_trader.pubkey()).await?;

    // Fund normal trader
    let recent_blockhash = banks_client.get_latest_blockhash().await?;
    let fund_tx = Transaction::new_signed_with_payer(
        &[system_instruction::transfer(
            &payer.pubkey(),
            &normal_trader.pubkey(),
            5_000_000_000, // 5 SOL
        )],
        Some(&payer.pubkey()),
        &[payer],
        recent_blockhash,
    );
    banks_client.process_transaction(fund_tx).await?;

    // Execute normal buy
    let buy_accounts = vec![
        AccountMeta::new(*manager, false),
        AccountMeta::new(normal_trader.pubkey(), true),
        AccountMeta::new(normal_token_account, false),
        AccountMeta::new(*pool_account, false),
        AccountMeta::new(*token_vault, false),
        AccountMeta::new(*sol_vault, false),
        AccountMeta::new_readonly(*raydium_program_id, false),
        AccountMeta::new_readonly(spl_token::id(), false),
        AccountMeta::new_readonly(system_program::id(), false),
        AccountMeta::new_readonly(sysvar::clock::id(), false),
    ];

    let mut buy_data = vec![0; 8 + 8];
    buy_data[0..8].copy_from_slice(&[242, 35, 198, 137, 82, 225, 242, 182]); // Buy discriminator
    buy_data[8..16].copy_from_slice(&500_000_000u64.to_le_bytes());

    let buy_ix = Instruction {
        program_id: abc_token::id(),
        accounts: buy_accounts,
        data: buy_data,
    };

    let recent_blockhash = banks_client.get_latest_blockhash().await?;
    let buy_tx = Transaction::new_signed_with_payer(
        &[buy_ix],
        Some(&payer.pubkey()),
        &[payer, &normal_trader],
        recent_blockhash,
    );

    banks_client.process_transaction(buy_tx).await?;

    // Verify normal trade
    let manager_account = banks_client.get_account(*manager).await?.unwrap();
    let manager_data = abc_token::ABCManager::try_deserialize(&mut &manager_account.data[..])?;

    assert_ne!(manager_data.last_blocked_address, normal_trader.pubkey());

    // Check token balance after buy
    let token_balance_after_buy = get_token_balance(banks_client, &normal_token_account).await?;
    assert!(token_balance_after_buy > 0);

    // Test selling
    let sell_accounts = vec![
        AccountMeta::new(*manager, false),
        AccountMeta::new(normal_trader.pubkey(), true),
        AccountMeta::new(normal_token_account, false),
        AccountMeta::new(*pool_account, false),
        AccountMeta::new(*token_vault, false),
        AccountMeta::new(*sol_vault, false),
        AccountMeta::new_readonly(*raydium_program_id, false),
        AccountMeta::new_readonly(spl_token::id(), false),
        AccountMeta::new_readonly(system_program::id(), false),
        AccountMeta::new_readonly(sysvar::clock::id(), false),
    ];

    let mut sell_data = vec![0; 8 + 8];
    sell_data[0..8].copy_from_slice(&[183, 18, 70, 156, 148, 109, 161, 34]); // Sell discriminator
    sell_data[8..16].copy_from_slice(&(token_balance_after_buy / 2).to_le_bytes());

    let sell_ix = Instruction {
        program_id: abc_token::id(),
        accounts: sell_accounts,
        data: sell_data,
    };

    let recent_blockhash = banks_client.get_latest_blockhash().await?;
    let sell_tx = Transaction::new_signed_with_payer(
        &[sell_ix],
        Some(&payer.pubkey()),
        &[payer, &normal_trader],
        recent_blockhash,
    );

    banks_client.process_transaction(sell_tx).await?;

    // Verify token balance after sell
    let final_token_balance = get_token_balance(banks_client, &normal_token_account).await?;
    assert_eq!(final_token_balance, token_balance_after_buy / 2);

    Ok(())
}

async fn get_token_balance(
    banks_client: &mut BanksClient,
    token_account: &Pubkey,
) -> TestResult<u64> {
    let account = banks_client.get_account(*token_account).await?.unwrap();
    let token_account = spl_token::state::Account::unpack(&account.data)
        .map_err(|e| TestError::PackError(e.to_string()))?;
    Ok(token_account.amount)
}

fn mock_raydium_processor(
    _program_id: &Pubkey,
    accounts: &[AccountInfo],
    instruction_data: &[u8],
) -> ProgramResult {
    if instruction_data.len() < 8 {
        return Err(ProgramError::InvalidInstructionData);
    }

    let (tag, _rest) = instruction_data.split_at(8);
    match tag {
        // Initialize pool instruction
        [1, 0, 0, 0, 0, 0, 0, 0] => {
            let accounts_iter = &mut accounts.iter();
            let authority = next_account_info(accounts_iter)?;
            let pool_account = next_account_info(accounts_iter)?;
            let token_vault = next_account_info(accounts_iter)?;
            let sol_vault = next_account_info(accounts_iter)?;

            // Basic validation
            if !authority.is_signer
                || !pool_account.is_writable
                || !token_vault.is_writable
                || !sol_vault.is_writable
            {
                return Err(ProgramError::InvalidAccountData);
            }

            Ok(())
        }

        // Swap instruction
        [2, 0, 0, 0, 0, 0, 0, 0] => {
            let accounts_iter = &mut accounts.iter();
            let trader = next_account_info(accounts_iter)?;
            let pool = next_account_info(accounts_iter)?;
            let token_vault = next_account_info(accounts_iter)?;
            let sol_vault = next_account_info(accounts_iter)?;
            let trader_token_account = next_account_info(accounts_iter)?;

            // Basic validation
            if !trader.is_signer
                || !pool.is_writable
                || !token_vault.is_writable
                || !sol_vault.is_writable
                || !trader_token_account.is_writable
            {
                return Err(ProgramError::InvalidAccountData);
            }

            Ok(())
        }

        _ => Err(ProgramError::InvalidInstructionData),
    }
}
