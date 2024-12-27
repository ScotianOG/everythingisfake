use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};
use std::str::FromStr;
use solana_program::{system_instruction, instruction::Instruction};
use anchor_lang::{solana_program::program::{invoke, invoke_signed}, prelude::Signer};

declare_id!("vBcHBCoQLGDvKejC5MHEZW4pLZi17FS8qPtyA2S6NVt");

// Constants moved to a separate section for better organization
pub mod constants {
    pub const MONITORING_BLOCKS: u64 = 5;
    pub const MIN_TRADE_SOL: u64 = 100_000; // 0.0001 SOL
    pub const MAX_TRADE_SOL: u64 = 1_000_000_000; // 1 SOL
    pub const RAYDIUM_PROGRAM_ID: &str = "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8";
    pub const MAX_PRICE_IMPACT_BPS: u64 = 1000; // 10%
    pub const SLIPPAGE_TOLERANCE_BPS: u64 = 100; // 1%
}

use constants::*;

// Program module with allow attribute to suppress Result size warning
#[program]
#[allow(clippy::result_large_err)]
pub mod abc_token {
    use super::*;

    pub fn initialize(ctx: Context<Initialize>, reserve_amount: u64) -> Result<()> {
        let manager = &mut ctx.accounts.manager;
        let clock = Clock::get()?;

        manager.initialize(
            ctx.accounts.authority.key(),
            ctx.accounts.mint.key(),
            clock.slot,
            reserve_amount,
            *ctx.bumps.get("manager").unwrap(),
            ctx.accounts.token_vault.key(),
        );

        // Transfer initial reserve tokens
        token::transfer(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.token_source.to_account_info(),
                    to: ctx.accounts.reserve_account.to_account_info(),
                    authority: ctx.accounts.authority.to_account_info(),
                },
            ),
            reserve_amount,
        )?;

        // Create Raydium pool with initial liquidity
        let raydium_init_ix = create_raydium_pool_ix(
            &Pubkey::from_str(RAYDIUM_PROGRAM_ID).unwrap(),
            &manager.mint,
            reserve_amount / 2, // 50% of reserve as initial liquidity
            MIN_TRADE_SOL,      // Minimum SOL liquidity
        )?;

        invoke(
            &raydium_init_ix,
            &[
                ctx.accounts.authority.to_account_info(),
                ctx.accounts.token_vault.to_account_info(),
                ctx.accounts.treasury.to_account_info(),
                ctx.accounts.token_program.to_account_info(),
                ctx.accounts.system_program.to_account_info(),
            ],
        )?;

        emit!(ProgramInitialized {
            launch_slot: manager.launch_slot,
            reserve_amount,
            raydium_pool: ctx.accounts.token_vault.key(),
        });

        Ok(())
    }

    // Buy entry point with cleaner error handling
    pub fn buy(ctx: Context<Trade>, sol_amount: u64) -> Result<()> {
        let clock = Clock::get()?;
        
        if ctx.accounts.manager.is_in_monitoring_period(clock.slot) {
            trade::process_monitored_buy(ctx, sol_amount)
        } else {
            trade::process_regular_buy(ctx, sol_amount)
        }
    }

    // Sell entry point with validation
    pub fn sell(ctx: Context<Trade>, token_amount: u64) -> Result<()> {
        require!(
            !ctx.accounts.manager.is_in_monitoring_period(Clock::get()?.slot),
            ErrorCode::TradingNotActive
        );

        trade::process_sell(ctx, token_amount)
    }
}

// Implementation methods for accounts
impl ABCManager {
    pub fn initialize(
        &mut self,
        authority: Pubkey,
        mint: Pubkey,
        launch_slot: u64,
        reserve_amount: u64,
        bump: u8,
        raydium_pool: Pubkey,
    ) {
        self.authority = authority;
        self.mint = mint;
        self.launch_slot = launch_slot;
        self.is_launched = true;
        self.captured_sol = 0;
        self.reserve_tokens = reserve_amount;
        self.bump = bump;
        self.last_blocked_address = Pubkey::default();
        self.raydium_pool = raydium_pool;
    }

    pub fn is_in_monitoring_period(&self, current_slot: u64) -> bool {
        current_slot <= self.launch_slot + MONITORING_BLOCKS
    }

    pub fn update_bot_capture(&mut self, bot_address: Pubkey, sol_amount: u64) -> Result<()> {
        self.last_blocked_address = bot_address;
        self.captured_sol = self.captured_sol
            .checked_add(sol_amount)
            .ok_or(ErrorCode::MathOverflow)?;
        Ok(())
    }
}

// Separate module for trading logic
mod trade {
    use super::*;

    pub fn process_regular_buy(ctx: Context<Trade>, sol_amount: u64) -> Result<()> {
        validate_trade_amount(sol_amount)?;

        // Execute trade through Raydium
        let raydium_swap_ix = create_raydium_swap_ix(
            &Pubkey::from_str(RAYDIUM_PROGRAM_ID).unwrap(),
            &ctx.accounts.token_vault.key(),
            sol_amount,
            true, // buying
        )?;

        invoke(
            &raydium_swap_ix,
            &[
                ctx.accounts.trader.to_account_info(),
                ctx.accounts.token_vault.to_account_info(),
                ctx.accounts.treasury.to_account_info(),
                ctx.accounts.system_program.to_account_info(),
            ],
        )?;

        let tokens_received = pricing::calculate_tokens_from_sol(sol_amount)?;

        emit!(TradeExecuted {
            trader: ctx.accounts.trader.key(),
            is_buy: true,
            sol_amount,
            token_amount: tokens_received,
            slot: Clock::get()?.slot,
        });

        Ok(())
    }

    pub fn process_monitored_buy(ctx: Context<Trade>, sol_amount: u64) -> Result<()> {
        ctx.accounts.manager.update_bot_capture(
            ctx.accounts.trader.key(),
            sol_amount,
        )?;

        let tokens_out = pricing::calculate_tokens_from_sol(sol_amount)?;

        // Transfer SOL from buyer
        let transfer_ix = system_instruction::transfer(
            &ctx.accounts.trader.key(),
            &ctx.accounts.treasury.key(),
            sol_amount,
        );

        invoke(
            &transfer_ix,
            &[
                ctx.accounts.trader.to_account_info(),
                ctx.accounts.treasury.to_account_info(),
                ctx.accounts.system_program.to_account_info(),
            ],
        )?;

        // Get manager signer seeds
        let seeds = [
            b"abc_manager".as_ref(),
            ctx.accounts.manager.mint.as_ref(),
            &[ctx.accounts.manager.bump]
        ];
        let signer_seeds = &[&seeds[..]];

        // Counter-trade through Raydium
        let raydium_swap_ix = create_raydium_swap_ix(
            &Pubkey::from_str(RAYDIUM_PROGRAM_ID).unwrap(),
            &ctx.accounts.token_vault.key(),
            tokens_out,
            false, // selling same amount
        )?;

        invoke_signed(
            &raydium_swap_ix,
            &[
                ctx.accounts.token_vault.to_account_info(),
                ctx.accounts.treasury.to_account_info(),
                ctx.accounts.system_program.to_account_info(),
            ],
            signer_seeds,
        )?;

        emit!(BotPurchaseHandled {
            bot_address: ctx.accounts.trader.key(),
            tokens_purchased: tokens_out,
            sol_captured: sol_amount,
            tokens_sold: tokens_out,
            slot: Clock::get()?.slot,
        });

        Ok(())
    }

    pub fn process_sell(ctx: Context<Trade>, token_amount: u64) -> Result<()> {
        let sol_out = pricing::calculate_sol_from_tokens(token_amount)?;
        validate_trade_amount(sol_out)?;

        // Execute sell through Raydium
        let raydium_swap_ix = create_raydium_swap_ix(
            &Pubkey::from_str(RAYDIUM_PROGRAM_ID).unwrap(),
            &ctx.accounts.token_vault.key(),
            token_amount,
            false, // selling
        )?;

        invoke(
            &raydium_swap_ix,
            &[
                ctx.accounts.trader.to_account_info(),
                ctx.accounts.token_vault.to_account_info(),
                ctx.accounts.treasury.to_account_info(),
                ctx.accounts.system_program.to_account_info(),
            ],
        )?;

        emit!(TradeExecuted {
            trader: ctx.accounts.trader.key(),
            is_buy: false,
            sol_amount: sol_out,
            token_amount,
            slot: Clock::get()?.slot,
        });

        Ok(())
    }

    fn validate_trade_amount(amount: u64) -> Result<()> {
        require!(amount >= MIN_TRADE_SOL, ErrorCode::TradeTooSmall);
        require!(amount <= MAX_TRADE_SOL, ErrorCode::TradeTooLarge);
        Ok(())
    }
}

// Separate module for pricing calculations
mod pricing {
    use super::*;

    pub fn calculate_tokens_from_sol(sol_amount: u64) -> Result<u64> {
        let sol_reserve: u128 = 1_000_000_000;
        let token_reserve: u128 = 1_000_000_000_000;
        
        calculate_swap_output(
            sol_amount,
            sol_reserve,
            token_reserve
        )
    }

    pub fn calculate_sol_from_tokens(token_amount: u64) -> Result<u64> {
        let sol_reserve: u128 = 1_000_000_000;
        let token_reserve: u128 = 1_000_000_000_000;
        
        calculate_swap_output(
            token_amount,
            token_reserve,
            sol_reserve
        )
    }

    fn calculate_swap_output(
        amount_in: u64,
        reserve_in: u128,
        reserve_out: u128
    ) -> Result<u64> {
        let k = reserve_in
            .checked_mul(reserve_out)
            .ok_or(ErrorCode::MathOverflow)?;

        let new_reserve_in = reserve_in
            .checked_add(amount_in as u128)
            .ok_or(ErrorCode::MathOverflow)?;

        let new_reserve_out = k
            .checked_div(new_reserve_in)
            .ok_or(ErrorCode::MathOverflow)?;

        let amount_out = reserve_out
            .checked_sub(new_reserve_out)
            .ok_or(ErrorCode::MathOverflow)?;

        Ok(amount_out as u64)
    }
}

// Helper functions for Raydium integration
fn create_raydium_pool_ix(
    program_id: &Pubkey,
    mint: &Pubkey,
    token_amount: u64,
    sol_amount: u64,
) -> Result<Instruction> {
    let mut data = Vec::with_capacity(40);
    data.extend_from_slice(&[1, 0, 0, 0, 0, 0, 0, 0]); // Initialize pool discriminator
    data.extend_from_slice(&token_amount.to_le_bytes());
    data.extend_from_slice(&sol_amount.to_le_bytes());
    data.extend_from_slice(mint.as_ref());

    Ok(Instruction {
        program_id: *program_id,
        accounts: vec![], // Accounts provided in invoke context
        data,
    })
}

fn create_raydium_swap_ix(
    program_id: &Pubkey,
    pool: &Pubkey,
    amount: u64,
    is_buy: bool,
) -> Result<Instruction> {
    let mut data = Vec::with_capacity(41);
    data.extend_from_slice(&[2, 0, 0, 0, 0, 0, 0, 0]); // Swap discriminator
    data.extend_from_slice(&amount.to_le_bytes());
    data.push(is_buy as u8);
    data.extend_from_slice(pool.as_ref());

    Ok(Instruction {
        program_id: *program_id,
        accounts: vec![], // Accounts provided in invoke context
        data,
    })
}

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,

    pub mint: Account<'info, Mint>,

    #[account(
        init,
        payer = authority,
        space = 8 + 32 + 32 + 8 + 1 + 8 + 8 + 1 + 32,
        seeds = [b"abc_manager", mint.key().as_ref()],
        bump
    )]
    pub manager: Account<'info, ABCManager>,

    #[account(
        mut,
        constraint = token_source.mint == mint.key(),
        constraint = token_source.owner == authority.key()
    )]
    pub token_source: Account<'info, TokenAccount>,

    #[account(
        init,
        payer = authority,
        seeds = [b"reserve", mint.key().as_ref()],
        bump,
        token::mint = mint,
        token::authority = manager,
    )]
    pub reserve_account: Account<'info, TokenAccount>,

    #[account(mut)]
    /// CHECK: Raydium pool account
    pub token_vault: AccountInfo<'info>,

    #[account(mut)]
    /// CHECK: Raydium SOL vault
    pub treasury: AccountInfo<'info>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
    pub clock: Sysvar<'info, Clock>,
}

#[derive(Accounts)]
pub struct Trade<'info> {
    #[account(mut)]
    pub manager: Account<'info, ABCManager>,

    #[account(mut)]
    pub trader: Signer<'info>,

    #[account(
        mut,
        constraint = trader_token_account.mint == manager.mint,
        constraint = trader_token_account.owner == trader.key()
    )]
    pub trader_token_account: Account<'info, TokenAccount>,

    #[account(
        mut,
        seeds = [b"reserve", manager.mint.as_ref()],
        bump,
        constraint = token_vault.key() == manager.raydium_pool
    )]
    pub token_vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        seeds = [b"treasury", manager.mint.as_ref()],
        bump
    )]
    /// CHECK: Treasury account for SOL
    pub treasury: AccountInfo<'info>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub clock: Sysvar<'info, Clock>,
}

#[account]
#[derive(Default)]
pub struct ABCManager {
    pub authority: Pubkey,
    pub mint: Pubkey,
    pub launch_slot: u64,
    pub is_launched: bool,
    pub captured_sol: u64,
    pub reserve_tokens: u64,
    pub bump: u8,
    pub last_blocked_address: Pubkey,
    pub raydium_pool: Pubkey,
}

#[event]
pub struct ProgramInitialized {
    pub launch_slot: u64,
    pub reserve_amount: u64,
    pub raydium_pool: Pubkey,
}

#[event]
pub struct BotPurchaseHandled {
    pub bot_address: Pubkey,
    pub tokens_purchased: u64,
    pub sol_captured: u64,
    pub tokens_sold: u64,
    pub slot: u64,
}

#[event]
pub struct TradeExecuted {
    pub trader: Pubkey,
    pub is_buy: bool,
    pub sol_amount: u64,
    pub token_amount: u64,
    pub slot: u64,
}

#[error_code]
pub enum ErrorCode {
    #[msg("Monitoring period has ended")]
    MonitoringPeriodEnded,

    #[msg("Math operation overflow")]
    MathOverflow,

    #[msg("Insufficient reserve balance")]
    InsufficientReserve,

    #[msg("Trading not yet active")]
    TradingNotActive,

    #[msg("Trade amount too small")]
    TradeTooSmall,

    #[msg("Trade amount too large")]
    TradeTooLarge,

    #[msg("Raydium pool not initialized")]
    RaydiumPoolNotInitialized,

    #[msg("Invalid Raydium program")]
    InvalidRaydiumProgram,

    #[msg("Slippage tolerance exceeded")]
    SlippageExceeded,
}

// Raydium pool state validation
#[derive(Accounts)]
pub struct ValidateRaydiumPool<'info> {
    pub manager: Account<'info, ABCManager>,

    /// CHECK: Validated in instruction
    #[account(mut)]
    pub pool_account: AccountInfo<'info>,

    #[account(mut)]
    pub token_vault: Account<'info, TokenAccount>,

    /// CHECK: Validated in instruction
    #[account(mut)]
    pub sol_vault: AccountInfo<'info>,
}

// Helper functions for Raydium pool validation
impl<'info> ValidateRaydiumPool<'info> {
    pub fn validate(&self) -> Result<()> {
        require!(
            self.pool_account.key() == self.manager.raydium_pool,
            ErrorCode::RaydiumPoolNotInitialized
        );

        require!(
            self.token_vault.owner == self.pool_account.key(),
            ErrorCode::InvalidRaydiumProgram
        );

        Ok(())
    }
}
