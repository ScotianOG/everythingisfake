use anchor_lang::prelude::*;
use solana_program_test::*;
use solana_sdk::{
    signature::{Keypair, Signer},
    system_instruction,
    transaction::Transaction,
};
use anchor_spl::token::{self, Mint, TokenAccount};

#[tokio::test]
async fn test_raydium_integration() {
    let program_id = abc_token::id();
    let raydium_program_id = Pubkey::new_unique(); // Mock Raydium program

    let mut program_test = ProgramTest::new(
        "abc_token",
        program_id,
        processor!(abc_token::entry),
    );

    // Add Raydium program
    program_test.add_program(
        "raydium",
        raydium_program_id,
        processor!(mock_raydium_processor),
    );

    // Start the test context
    let (mut banks_client, payer, recent_blockhash) = program_test.start().await;

    // Create test accounts
    let mint_keypair = Keypair::new();
    let authority = Keypair::new();

    // Fund accounts
    banks_client.process_transaction(Transaction::new_signed_with_payer(
        &[system_instruction::transfer(
            &payer.pubkey(),
            &authority.pubkey(),
            100_000_000_000, // 100 SOL
        )],
        Some(&payer.pubkey()),
        &[&payer],
        recent_blockhash,
    )).await.unwrap();

    // Set up mint and token accounts
    let (token_source, token_vault, sol_vault, pool_account) = setup_token_accounts(
        &mut banks_client,
        &payer,
        &mint_keypair,
        &authority,
        &raydium_program_id,
    ).await;

    // Test 1: Initialize with Raydium pool
    let (manager, reserve_account) = test_initialization(
        &mut banks_client,
        &payer,
        &authority,
        &mint_keypair,
        &token_source,
        &token_vault,
        &sol_vault,
        &pool_account,
        &raydium_program_id,
    ).await;

    // Test 2: Bot detection during monitoring period
    test_bot_detection(
        &mut banks_client,
        &payer,
        &manager,
        &pool_account,
        &token_vault,
        &sol_vault,
        &mint_keypair,
        &raydium_program_id,
    ).await;

    // Test 3: Normal trading after monitoring period
    test_normal_trading(
        &mut banks_client,
        &payer,
        &manager,
        &pool_account,
        &token_vault,
        &sol_vault,
        &mint_keypair,
        &raydium_program_id,
    ).await;
}

async fn setup_token_accounts(
    banks_client: &mut BanksClient,
    payer: &Keypair,
    mint_keypair: &Keypair,
    authority: &Keypair,
    raydium_program_id: &Pubkey,
) -> (Pubkey, Pubkey, Pubkey, Pubkey) {
    // Create mint
    let rent = banks_client.get_rent().await.unwrap();
    let mint_rent = rent.minimum_balance(spl_token::state::Mint::LEN);
    
    let create_mint_ix = system_instruction::create_account(
        &payer.pubkey(),
        &mint_keypair.pubkey(),
        mint_rent,
        spl_token::state::Mint::LEN as u64,
        &spl_token::id(),
    );

    banks_client.process_transaction(Transaction::new_signed_with_payer(
        &[create_mint_ix],
        Some(&payer.pubkey()),
        &[payer, mint_keypair],
        banks_client.get_recent_blockhash().await.unwrap(),
    )).await.unwrap();

    // Initialize mint
    let init_mint_ix = spl_token::instruction::initialize_mint(
        &spl_token::id(),
        &mint_keypair.pubkey(),
        &authority.pubkey(),
        None,
        9,
    ).unwrap();

    banks_client.process_transaction(Transaction::new_signed_with_payer(
        &[init_mint_ix],
        Some(&payer.pubkey()),
        &[payer],
        banks_client.get_recent_blockhash().await.unwrap(),
    )).await.unwrap();

    // Create token source account
    let token_source = Keypair::new();
    let account_rent = rent.minimum_balance(spl_token::state::Account::LEN);

    let create_source_ix = system_instruction::create_account(
        &payer.pubkey(),
        &token_source.pubkey(),
        account_rent,
        spl_token::state::Account::LEN as u64,
        &spl_token::id(),
    );

    let init_source_ix = spl_token::instruction::initialize_account(
        &spl_token::id(),
        &token_source.pubkey(),
        &mint_keypair.pubkey(),
        &authority.pubkey(),
    ).unwrap();

    banks_client.process_transaction(Transaction::new_signed_with_payer(
        &[create_source_ix, init_source_ix],
        Some(&payer.pubkey()),
        &[payer, &token_source],
        banks_client.get_recent_blockhash().await.unwrap(),
    )).await.unwrap();

    // Create Raydium pool accounts
    let pool_account = Keypair::new();
    let token_vault = Keypair::new();
    let sol_vault = Keypair::new();

    // Initialize pool accounts
    let pool_rent = rent.minimum_balance(1000); // Mock size for pool account
    let vault_rent = rent.minimum_balance(spl_token::state::Account::LEN);

    // Create pool account
    banks_client.process_transaction(Transaction::new_signed_with_payer(
        &[system_instruction::create_account(
            &payer.pubkey(),
            &pool_account.pubkey(),
            pool_rent,
            1000,
            raydium_program_id,
        )],
        Some(&payer.pubkey()),
        &[payer, &pool_account],
        banks_client.get_recent_blockhash().await.unwrap(),
    )).await.unwrap();

    // Create and initialize token vault
    let create_token_vault_ix = system_instruction::create_account(
        &payer.pubkey(),
        &token_vault.pubkey(),
        vault_rent,
        spl_token::state::Account::LEN as u64,
        &spl_token::id(),
    );

    let init_token_vault_ix = spl_token::instruction::initialize_account(
        &spl_token::id(),
        &token_vault.pubkey(),
        &mint_keypair.pubkey(),
        &pool_account.pubkey(),
    ).unwrap();

    banks_client.process_transaction(Transaction::new_signed_with_payer(
        &[create_token_vault_ix, init_token_vault_ix],
        Some(&payer.pubkey()),
        &[payer, &token_vault],
        banks_client.get_recent_blockhash().await.unwrap(),
    )).await.unwrap();

    // Create and initialize SOL vault
    let create_sol_vault_ix = system_instruction::create_account(
        &payer.pubkey(),
        &sol_vault.pubkey(),
        vault_rent,
        spl_token::state::Account::LEN as u64,
        &spl_token::id(),
    );

    let init_sol_vault_ix = spl_token::instruction::initialize_account(
        &spl_token::id(),
        &sol_vault.pubkey(),
        &mint_keypair.pubkey(),
        &pool_account.pubkey(),
    ).unwrap();

    banks_client.process_transaction(Transaction::new_signed_with_payer(
        &[create_sol_vault_ix, init_sol_vault_ix],
        Some(&payer.pubkey()),
        &[payer, &sol_vault],
        banks_client.get_recent_blockhash().await.unwrap(),
    )).await.unwrap();

    (
        token_source.pubkey(),
        token_vault.pubkey(),
        sol_vault.pubkey(),
        pool_account.pubkey(),
    )
}

// Mock Raydium processor for testing
fn mock_raydium_processor(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    instruction_data: &[u8],
) -> ProgramResult {
    // Mock pool creation
    if instruction_data[0..8] == [147, 216, 24, 218, 163, 45, 141, 86] {
        // Extract pool creation parameters
        let sol_amount = u64::from_le_bytes(instruction_data[8..16].try_into().unwrap());
        let token_amount = u64::from_le_bytes(instruction_data[16..24].try_into().unwrap());
        
        // Mock pool initialization
        let pool_account = &accounts[1];
        let token_vault = &accounts[2];
        let sol_vault = &accounts[3];
        
        // In real Raydium, this would create pool state and initialize vaults
        // For testing, we just need to verify the accounts exist
        assert!(pool_account.is_writable);
        assert!(token_vault.is_writable);
        assert!(sol_vault.is_writable);
        
        Ok(())
    }
    // Mock swap
    else if instruction_data[0..8] == [123, 98, 207, 75, 88, 145, 154, 23] {
        // Extract swap parameters
        let amount = u64::from_le_bytes(instruction_data[8..16].try_into().unwrap());
        let is_buy = instruction_data[16] != 0;
        
        // Mock swap execution
        let trader_account = &accounts[0];
        let pool_account = &accounts[1];
        let token_vault = &accounts[2];
        let sol_vault = &accounts[3];
        let trader_token_account = &accounts[4];
        
        // Verify account permissions
        assert!(trader_account.is_signer);
        assert!(pool_account.is_writable);
        assert!(token_vault.is_writable);
        assert!(sol_vault.is_writable);
        assert!(trader_token_account.is_writable);
        
        // In real Raydium, this would perform the actual swap
        // For testing, we just verify the accounts are properly set up
        Ok(())
    } else {
        Err(ProgramError::InvalidInstructionData)
    }
}

#[tokio::test]
async fn test_sell_flow() {
    let program_id = abc_token::id();
    let raydium_program_id = Pubkey::new_unique();
    
    let mut program_test = ProgramTest::new(
        "abc_token",
        program_id,
        processor!(abc_token::entry),
    );

    program_test.add_program(
        "raydium",
        raydium_program_id,
        processor!(mock_raydium_processor),
    );

    let (mut banks_client, payer, recent_blockhash) = program_test.start().await;

    // Set up test accounts and pool
    let (manager, pool_account, token_vault, sol_vault, trader_token_account) = 
        setup_full_test_state(&mut banks_client, &payer, &raydium_program_id).await;

    // Advance past monitoring period
    banks_client.advance_clock(6).await;

    // Test regular sell
    let sell_amount = 1_000_000_000; // 1000 tokens
    let trader = Keypair::new();
    
    let sell_ix = Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new(manager, false),
            AccountMeta::new(trader.pubkey(), true),
            AccountMeta::new(trader_token_account, false),
            AccountMeta::new(pool_account, false),
            AccountMeta::new(token_vault, false),
            AccountMeta::new(sol_vault, false),
            AccountMeta::new_readonly(raydium_program_id, false),
            AccountMeta::new_readonly(spl_token::id(), false),
            AccountMeta::new_readonly(system_program::id(), false),
            AccountMeta::new_readonly(sysvar::clock::id(), false),
        ],
        data: abc_token::instruction::Sell {
            token_amount: sell_amount,
        }.data(),
    };

    let tx = Transaction::new_signed_with_payer(
        &[sell_ix],
        Some(&payer.pubkey()),
        &[&payer, &trader],
        recent_blockhash,
    );

    banks_client.process_transaction(tx).await.unwrap();

    // Verify sell
    let token_account = banks_client.get_account(trader_token_account)
        .await.unwrap().unwrap();
    let token_balance = spl_token::state::Account::unpack(&token_account.data)
        .unwrap().amount;
    assert_eq!(token_balance, 0); // All tokens sold
}

async fn setup_full_test_state(
    banks_client: &mut BanksClient,
    payer: &Keypair,
    raydium_program_id: &Pubkey,
) -> (Pubkey, Pubkey, Pubkey, Pubkey, Pubkey) {
    // Create all necessary accounts for a complete test
    let mint_keypair = Keypair::new();
    let authority = Keypair::new();

    // Fund authority
    banks_client.process_transaction(Transaction::new_signed_with_payer(
        &[system_instruction::transfer(
            &payer.pubkey(),
            &authority.pubkey(),
            100_000_000_000,
        )],
        Some(&payer.pubkey()),
        &[payer],
        banks_client.get_recent_blockhash().await.unwrap(),
    )).await.unwrap();

    // Set up token accounts
    let (token_source, token_vault, sol_vault, pool_account) = setup_token_accounts(
        banks_client,
        payer,
        &mint_keypair,
        &authority,
        raydium_program_id,
    ).await;

    // Initialize program
    let (manager, _) = test_initialization(
        banks_client,
        payer,
        &authority,
        &mint_keypair,
        &token_source,
        &token_vault,
        &sol_vault,
        &pool_account,
        raydium_program_id,
    ).await;

    // Create trader token account
    let trader_token_account = create_token_account(
        banks_client,
        payer,
        &mint_keypair,
        &Keypair::new().pubkey(),
    ).await;

    (manager, pool_account, token_vault, sol_vault, trader_token_account)
}

async fn test_initialization(
    banks_client: &mut BanksClient,
    payer: &Keypair,
    authority: &Keypair,
    mint_keypair: &Keypair,
    token_source: &Pubkey,
    token_vault: &Pubkey,
    sol_vault: &Pubkey,
    pool_account: &Pubkey,
    raydium_program_id: &Pubkey,
) -> (Pubkey, Pubkey) {
    // Calculate PDAs
    let (manager, _) = Pubkey::find_program_address(
        &[b"abc_manager", mint_keypair.pubkey().as_ref()],
        &abc_token::id(),
    );

    let (reserve_account, _) = Pubkey::find_program_address(
        &[b"reserve", mint_keypair.pubkey().as_ref()],
        &abc_token::id(),
    );

    // Create initialization instruction
    let init_ix = Instruction {
        program_id: abc_token::id(),
        accounts: vec![
            AccountMeta::new(authority.pubkey(), true),
            AccountMeta::new_readonly(mint_keypair.pubkey(), false),
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
        ],
        data: abc_token::instruction::Initialize {
            reserve_amount: 1_000_000_000_000, // 1M tokens
        }.data(),
    };

    let tx = Transaction::new_signed_with_payer(
        &[init_ix],
        Some(&payer.pubkey()),
        &[payer, authority],
        banks_client.get_recent_blockhash().await.unwrap(),
    );

    banks_client.process_transaction(tx).await.unwrap();

    // Verify initialization
    let manager_account = banks_client.get_account(manager).await.unwrap().unwrap();
    let manager_data = abc_token::ABCManager::try_deserialize(&mut &manager_account.data[..]).unwrap();
    
    assert_eq!(manager_data.authority, authority.pubkey());
    assert_eq!(manager_data.mint, mint_keypair.pubkey());
    assert!(manager_data.is_launched);
    assert_eq!(manager_data.raydium_pool, *pool_account);

    (manager, reserve_account)
}

async fn test_bot_detection(
    banks_client: &mut BanksClient,
    payer: &Keypair,
    manager: &Pubkey,
    pool_account: &Pubkey,
    token_vault: &Pubkey,
    sol_vault: &Pubkey,
    mint_keypair: &Keypair,
    raydium_program_id: &Pubkey,
) {
    let bot_trader = Keypair::new();
    let bot_token_account = create_token_account(
        banks_client,
        payer,
        mint_keypair,
        &bot_trader.pubkey(),
    ).await;

    // Fund bot trader
    banks_client.process_transaction(Transaction::new_signed_with_payer(
        &[system_instruction::transfer(
            &payer.pubkey(),
            &bot_trader.pubkey(),
            5_000_000_000, // 5 SOL
        )],
        Some(&payer.pubkey()),
        &[payer],
        banks_client.get_recent_blockhash().await.unwrap(),
    )).await.unwrap();

    // Execute trade during monitoring period
    let buy_ix = Instruction {
        program_id: abc_token::id(),
        accounts: vec![
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
        ],
        data: abc_token::instruction::Buy {
            sol_amount: 1_000_000_000, // 1 SOL
        }.data(),
    };

    let tx = Transaction::new_signed_with_payer(
        &[buy_ix],
        Some(&payer.pubkey()),
        &[payer, &bot_trader],
        banks_client.get_recent_blockhash().await.unwrap(),
    );

    banks_client.process_transaction(tx).await.unwrap();

    // Verify bot detection
    let manager_data = abc_token::ABCManager::try_deserialize(
        &mut &banks_client.get_account(*manager).await.unwrap().unwrap().data[..]
    ).unwrap();
    
    assert_eq!(manager_data.last_blocked_address, bot_trader.pubkey());
    assert_eq!(manager_data.captured_sol, 1_000_000_000);
}

async fn test_normal_trading(
    banks_client: &mut BanksClient,
    payer: &Keypair,
    manager: &Pubkey,
    pool_account: &Pubkey,
    token_vault: &Pubkey,
    sol_vault: &Pubkey,
    mint_keypair: &Keypair,
    raydium_program_id: &Pubkey,
) {
    // Advance clock past monitoring period
    banks_client.advance_clock(6).await;

    let normal_trader = Keypair::new();
    let normal_token_account = create_token_account(
        banks_client,
        payer,
        mint_keypair,
        &normal_trader.pubkey(),
    ).await;

    // Fund normal trader
    banks_client.process_transaction(Transaction::new_signed_with_payer(
        &[system_instruction::transfer(
            &payer.pubkey(),
            &normal_trader.pubkey(),
            5_000_000_000, // 5 SOL
        )],
        Some(&payer.pubkey()),
        &[payer],
        banks_client.get_recent_blockhash().await.unwrap(),
    )).await.unwrap();

    // Execute normal buy
    let normal_buy_ix = Instruction {
        program_id: abc_token::id(),
        accounts: vec![
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
        ],
        data: abc_token::instruction::Buy {
            sol_amount: 500_000_000, // 0.5 SOL
        }.data(),
    );

    let tx = Transaction::new_signed_with_payer(
        &[normal_buy_ix],
        Some(&payer.pubkey()),
        &[payer, &normal_trader],
        banks_client.get_recent_blockhash().await.unwrap(),
    );

    banks_client.process_transaction(tx).await.unwrap();

    // Verify normal trade
    let manager_data = abc_token::ABCManager::try_deserialize(
        &mut &banks_client.get_account(*manager).await.unwrap().unwrap().data[..]
    ).unwrap();
    
    assert_ne!(manager_data.last_blocked_address, normal_trader.pubkey());

    // Get token balance
    let token_account = banks_client.get_account(normal_token_account)
        .await.unwrap().unwrap();
    let token_balance = spl_token::state::Account::unpack(&token_account.data)
        .unwrap().amount;
    assert!(token_balance > 0);
}

async fn create_token_account(
    banks_client: &mut BanksClient,
    payer: &Keypair,
    mint: &Keypair,
    owner: &Pubkey,
) -> Pubkey {
    let token_account = Keypair::new();
    let rent = banks_client.get_rent().await.unwrap();
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
        &mint.pubkey(),
        owner,
    ).unwrap();

    let tx = Transaction::new_signed_with_payer(
        &[create_account_ix, initialize_account_ix],
        Some(&payer.pubkey()),
        &[payer, &token_account],
        banks_client.get_recent_blockhash().await.unwrap(),
    );

    banks_client.process_transaction(tx).await.unwrap();
    token_account.pubkey()
}
