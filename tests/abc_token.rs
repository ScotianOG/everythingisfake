use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount};
use solana_program_test::*;
use solana_sdk::{
    signature::{Keypair, Signer},
    system_instruction,
    transaction::Transaction,
};

pub mod tests {
    use super::*;

    struct TestContext {
        banks_client: BanksClient,
        payer: Keypair,
        recent_blockhash: Hash,
        program_id: Pubkey,
        mint: Keypair,
        authority: Keypair,
        manager: Pubkey,
        reserve_account: Pubkey,
        token_source: Keypair,
        amm_token_account: Keypair,
        bump: u8,
    }

    impl TestContext {
        async fn new() -> Self {
            let program_id = abc_token::id();
            let mut program_test = ProgramTest::new(
                "abc_token",
                program_id,
                processor!(abc_token::entry),
            );

            let (banks_client, payer, recent_blockhash) = program_test.start().await;
            
            let mint = Keypair::new();
            let authority = Keypair::new();
            let token_source = Keypair::new();
            let amm_token_account = Keypair::new();

            let (manager, bump) = Pubkey::find_program_address(
                &[b"abc_manager", mint.pubkey().as_ref()],
                &program_id,
            );

            let (reserve_account, _) = Pubkey::find_program_address(
                &[b"reserve", mint.pubkey().as_ref()],
                &program_id,
            );

            Self {
                banks_client,
                payer,
                recent_blockhash,
                program_id,
                mint,
                authority,
                manager,
                reserve_account,
                token_source,
                amm_token_account,
                bump,
            }
        }

        async fn setup_token_accounts(&mut self) -> Result<(), BanksClientError> {
            let rent = self.banks_client.get_rent().await.unwrap();
            
            // Create mint account
            let mint_rent = rent.minimum_balance(spl_token::state::Mint::LEN);
            let create_mint_ix = system_instruction::create_account(
                &self.payer.pubkey(),
                &self.mint.pubkey(),
                mint_rent,
                spl_token::state::Mint::LEN as u64,
                &spl_token::id(),
            );

            let init_mint_ix = spl_token::instruction::initialize_mint(
                &spl_token::id(),
                &self.mint.pubkey(),
                &self.authority.pubkey(),
                None,
                9,
            ).unwrap();

            let tx = Transaction::new_signed_with_payer(
                &[create_mint_ix, init_mint_ix],
                Some(&self.payer.pubkey()),
                &[&self.payer, &self.mint],
                self.recent_blockhash,
            );
            self.banks_client.process_transaction(tx).await?;

            // Create token source account
            let account_rent = rent.minimum_balance(spl_token::state::Account::LEN);
            let create_source_ix = system_instruction::create_account(
                &self.payer.pubkey(),
                &self.token_source.pubkey(),
                account_rent,
                spl_token::state::Account::LEN as u64,
                &spl_token::id(),
            );

            let init_source_ix = spl_token::instruction::initialize_account(
                &spl_token::id(),
                &self.token_source.pubkey(),
                &self.mint.pubkey(),
                &self.authority.pubkey(),
            ).unwrap();

            let tx = Transaction::new_signed_with_payer(
                &[create_source_ix, init_source_ix],
                Some(&self.payer.pubkey()),
                &[&self.payer, &self.token_source],
                self.recent_blockhash,
            );
            self.banks_client.process_transaction(tx).await?;

            // Create AMM token account
            let create_amm_ix = system_instruction::create_account(
                &self.payer.pubkey(),
                &self.amm_token_account.pubkey(),
                account_rent,
                spl_token::state::Account::LEN as u64,
                &spl_token::id(),
            );

            let init_amm_ix = spl_token::instruction::initialize_account(
                &spl_token::id(),
                &self.amm_token_account.pubkey(),
                &self.mint.pubkey(),
                &self.authority.pubkey(),
            ).unwrap();

            let tx = Transaction::new_signed_with_payer(
                &[create_amm_ix, init_amm_ix],
                Some(&self.payer.pubkey()),
                &[&self.payer, &self.amm_token_account],
                self.recent_blockhash,
            );
            self.banks_client.process_transaction(tx).await?;

            Ok(())
        }

        async fn mint_tokens(&mut self, amount: u64) -> Result<(), BanksClientError> {
            let ix = spl_token::instruction::mint_to(
                &spl_token::id(),
                &self.mint.pubkey(),
                &self.token_source.pubkey(),
                &self.authority.pubkey(),
                &[],
                amount,
            ).unwrap();

            let tx = Transaction::new_signed_with_payer(
                &[ix],
                Some(&self.payer.pubkey()),
                &[&self.payer, &self.authority],
                self.recent_blockhash,
            );

            self.banks_client.process_transaction(tx).await
        }

        async fn initialize_contract(&mut self, reserve_amount: u64) -> Result<(), BanksClientError> {
            let accounts = abc_token::accounts::Initialize {
                authority: self.authority.pubkey(),
                mint: self.mint.pubkey(),
                manager: self.manager,
                token_source: self.token_source.pubkey(),
                reserve_account: self.reserve_account,
                token_program: token::ID,
                system_program: system_program::ID,
            };

            let ix = abc_token::instruction::Initialize {
                reserve_amount,
            };

            let tx = Transaction::new_signed_with_payer(
                &[ix.instruction(accounts)],
                Some(&self.payer.pubkey()),
                &[&self.payer, &self.authority],
                self.recent_blockhash,
            );

            self.banks_client.process_transaction(tx).await
        }
    }

    #[tokio::test]
    async fn test_initialization() {
        let mut ctx = TestContext::new().await;
        ctx.setup_token_accounts().await.unwrap();

        // Test with 1M tokens, 40% reserve
        let initial_supply = 1_000_000 * 10u64.pow(9);
        ctx.mint_tokens(initial_supply).await.unwrap();
        
        let reserve_amount = initial_supply * 40 / 100;
        ctx.initialize_contract(reserve_amount).await.unwrap();

        // Verify initialization
        let manager_account = ctx.banks_client
            .get_account(ctx.manager)
            .await
            .unwrap()
            .unwrap();

        let manager = abc_token::ABCManager::try_deserialize(
            &mut manager_account.data.as_ref()
        ).unwrap();

        assert_eq!(manager.authority, ctx.authority.pubkey());
        assert_eq!(manager.mint, ctx.mint.pubkey());
        assert_eq!(manager.reserve_tokens, reserve_amount);
        assert!(manager.is_launched);
        assert_eq!(manager.captured_sol, 0);
        assert!(manager.blacklisted.is_empty());
        assert_eq!(manager.bump, ctx.bump);
    }

    #[tokio::test]
    async fn test_bot_detection() {
        let mut ctx = TestContext::new().await;
        ctx.setup_token_accounts().await.unwrap();

        let initial_supply = 1_000_000 * 10u64.pow(9);
        ctx.mint_tokens(initial_supply).await.unwrap();
        
        let reserve_amount = initial_supply * 40 / 100;
        ctx.initialize_contract(reserve_amount).await.unwrap();

        // Simulate bot purchase
        let bot_wallet = Keypair::new();
        let purchase_amount = 1_000 * 10u64.pow(9);
        let sol_spent = 1 * 10u64.pow(9);

        let accounts = abc_token::accounts::HandleBotPurchase {
            manager: ctx.manager,
            bot_address: bot_wallet.pubkey(),
            reserve_account: ctx.reserve_account,
            amm_token_account: ctx.amm_token_account.pubkey(),
            token_program: token::ID,
        };

        let ix = abc_token::instruction::HandleBotPurchase {
            purchase_amount,
            sol_spent,
        };

        let tx = Transaction::new_signed_with_payer(
            &[ix.instruction(accounts)],
            Some(&ctx.payer.pubkey()),
            &[&ctx.payer],
            ctx.recent_blockhash,
        );

        ctx.banks_client.process_transaction(tx).await.unwrap();

        // Verify bot handling
        let manager_account = ctx.banks_client
            .get_account(ctx.manager)
            .await
            .unwrap()
            .unwrap();

        let manager = abc_token::ABCManager::try_deserialize(
            &mut manager_account.data.as_ref()
        ).unwrap();

        assert!(manager.blacklisted.contains(&bot_wallet.pubkey()));
        assert_eq!(manager.captured_sol, sol_spent);
        assert_eq!(
            manager.reserve_tokens,
            reserve_amount - purchase_amount
        );
    }

    #[tokio::test]
    async fn test_monitoring_period() {
        let mut ctx = TestContext::new().await;
        ctx.setup_token_accounts().await.unwrap();

        let initial_supply = 1_000_000 * 10u64.pow(9);
        ctx.mint_tokens(initial_supply).await.unwrap();
        
        let reserve_amount = initial_supply * 40 / 100;
        ctx.initialize_contract(reserve_amount).await.unwrap();

        // Advance clock beyond monitoring period (5 blocks)
        ctx.banks_client.advance_clock(6).await;

        // Attempt bot purchase after monitoring period
        let bot_wallet = Keypair::new();
        let accounts = abc_token::accounts::HandleBotPurchase {
            manager: ctx.manager,
            bot_address: bot_wallet.pubkey(),
            reserve_account: ctx.reserve_account,
            amm_token_account: ctx.amm_token_account.pubkey(),
            token_program: token::ID,
        };

        let ix = abc_token::instruction::HandleBotPurchase {
            purchase_amount: 1_000 * 10u64.pow(9),
            sol_spent: 1 * 10u64.pow(9),
        };

        let tx = Transaction::new_signed_with_payer(
            &[ix.instruction(accounts)],
            Some(&ctx.payer.pubkey()),
            &[&ctx.payer],
            ctx.recent_blockhash,
        );

        let result = ctx.banks_client.process_transaction(tx).await;
        assert!(matches!(
            result,
            Err(BanksClientError::TransactionError(
                TransactionError::InstructionError(
                    _,
                    InstructionError::Custom(6000) // MonitoringPeriodEnded
                )
            ))
        ));
    }
}