use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount};
use solana_program::{program::invoke, program::invoke_signed, instruction::Instruction};

declare_id!("11111111111111111111111111111111");

#[program]
pub mod abc_token {
    use super::*;

    // Constants
    const MONITORING_BLOCKS: u64 = 100;
    const MIN_TRADE_SOL: u64 = 100_000_000; // 0.1 SOL
    const MAX_TRADE_SOL: u64 = 10_000_000_000; // 10 SOL

    // Modified buy and sell functions
    pub fn buy(ctx: Context<Trade>, sol_amount: u64) -> Result<()> {
        let clock = Clock::get()?;
        let current_slot = clock.slot;
        let manager = &mut ctx.accounts.manager;
        
        // Create Raydium swap instruction for initial buy
        let buy_ix = create_raydium_swap_ix(
            &ctx.accounts.raydium_program.key(),
            &manager.raydium_pool,
            sol_amount,
            true, // is_buy
        )?;

        // Execute initial buy through Raydium
        solana_program::program::invoke(
            &buy_ix,
            &[
                ctx.accounts.trader.to_account_info(),
                ctx.accounts.raydium_pool.to_account_info(),
                ctx.accounts.raydium_token_vault.to_account_info(),
                ctx.accounts.raydium_sol_vault.to_account_info(),
                ctx.accounts.trader_token_account.to_account_info(),
                ctx.accounts.raydium_program.to_account_info(),
            ],
        )?;

        // Check if in monitoring period
        if current_slot <= manager.launch_slot + MONITORING_BLOCKS {
            // Mark as bot
            manager.last_blocked_address = ctx.accounts.trader.key();
            
            // Track captured SOL
            manager.captured_sol = manager.captured_sol
                .checked_add(sol_amount)
                .ok_or(ErrorCode::MathOverflow)?;

            // Get manager signer seeds for counter-trade
            let bump = manager.bump;
            let mint_key = manager.mint.key();
            let seeds = &[b"abc_manager", mint_key.as_ref(), &[bump]];
            let signer = &[&seeds[..]];

            // Calculate approximate tokens received by bot
            let tokens_received = estimate_received_tokens(sol_amount)?;

            // Create counter-trade instruction
            let sell_ix = create_raydium_swap_ix(
                &ctx.accounts.raydium_program.key(),
                &manager.raydium_pool,
                tokens_received,
                false, // is_buy (we're selling)
            )?;

            // Execute counter-trade from reserves
            solana_program::program::invoke_signed(
                &sell_ix,
                &[
                    ctx.accounts.manager.to_account_info(),
                    ctx.accounts.raydium_pool.to_account_info(),
                    ctx.accounts.raydium_token_vault.to_account_info(),
                    ctx.accounts.raydium_sol_vault.to_account_info(),
                    ctx.accounts.token_vault.to_account_info(),
                    ctx.accounts.raydium_program.to_account_info(),
                ],
                signer,
            )?;

            emit!(BotPurchaseHandled {
                bot_address: ctx.accounts.trader.key(),
                tokens_purchased: tokens_received,
                sol_captured: sol_amount,
                tokens_sold: tokens_received,
            });

            return Ok(());
        }
        
        // Regular trade validation
        require!(sol_amount >= MIN_TRADE_SOL, ErrorCode::TradeTooSmall);
        require!(sol_amount <= MAX_TRADE_SOL, ErrorCode::TradeTooLarge);

        emit!(TradeExecuted {
            trader: ctx.accounts.trader.key(),
            is_buy: true,
            sol_amount,
            token_amount: estimate_received_tokens(sol_amount)?,
        });

        Ok(())
    }

    pub fn sell(ctx: Context<Trade>, token_amount: u64) -> Result<()> {
        let manager = &ctx.accounts.manager;
        let clock = Clock::get()?;
        let current_slot = clock.slot;
        
        require!(
            current_slot > manager.launch_slot + MONITORING_BLOCKS,
            ErrorCode::TradingNotActive
        );

        let estimated_sol = estimate_received_sol(token_amount)?;
        require!(estimated_sol >= MIN_TRADE_SOL, ErrorCode::TradeTooSmall);
        require!(estimated_sol <= MAX_TRADE_SOL, ErrorCode::TradeTooLarge);

        // Create Raydium swap instruction
        let sell_ix = create_raydium_swap_ix(
            &ctx.accounts.raydium_program.key(),
            &manager.raydium_pool,
            token_amount,
            false, // is_buy
        )?;

        // Execute sell through Raydium
        solana_program::program::invoke(
            &sell_ix,
            &[
                ctx.accounts.trader.to_account_info(),
                ctx.accounts.raydium_pool.to_account_info(),
                ctx.accounts.raydium_token_vault.to_account_info(),
                ctx.accounts.raydium_sol_vault.to_account_info(),
                ctx.accounts.trader_token_account.to_account_info(),
                ctx.accounts.raydium_program.to_account_info(),
            ],
        )?;

        emit!(TradeExecuted {
            trader: ctx.accounts.trader.key(),
            is_buy: false,
            sol_amount: estimated_sol,
            token_amount,
        });

        Ok(())
    }
}

// Account structs
#[account]
pub struct ABCManager {
    pub authority: Pubkey,
    pub mint: Pubkey,
    pub is_launched: bool,
    pub launch_slot: u64,
    pub raydium_pool: Pubkey,
    pub last_blocked_address: Pubkey,
    pub captured_sol: u64,
    pub bump: u8,
}

// Instruction contexts
#[derive(Accounts)]
pub struct Trade<'info> {
    #[account(mut)]
    pub manager: Account<'info, ABCManager>,
    #[account(mut)]
    pub trader: Signer<'info>,
    #[account(mut)]
    pub trader_token_account: Account<'info, TokenAccount>,
    #[account(mut)]
    pub raydium_pool: AccountInfo<'info>,
    #[account(mut)]
    pub raydium_token_vault: Account<'info, TokenAccount>,
    #[account(mut)]
    pub raydium_sol_vault: Account<'info, TokenAccount>,
    #[account(mut)]
    pub token_vault: Account<'info, TokenAccount>,
    pub raydium_program: AccountInfo<'info>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub clock: Sysvar<'info, Clock>,
}

// Event structs
#[event]
pub struct TradeExecuted {
    pub trader: Pubkey,
    pub is_buy: bool,
    pub sol_amount: u64,
    pub token_amount: u64,
}

#[event]
pub struct BotPurchaseHandled {
    pub bot_address: Pubkey,
    pub tokens_purchased: u64,
    pub sol_captured: u64,
    pub tokens_sold: u64,
}

// Error codes
#[error_code]
pub enum ErrorCode {
    #[msg("Trade amount is too small")]
    TradeTooSmall,
    #[msg("Trade amount is too large")]
    TradeTooLarge,
    #[msg("Trading is not yet active")]
    TradingNotActive,
    #[msg("Math overflow")]
    MathOverflow,
}

// Helper structs
pub struct RaydiumPoolParams {
    pub sol_amount: u64,
    pub token_amount: u64,
    pub initial_price: u64,
    pub token_mint: Pubkey,
}

// Raydium instruction creation implementations
fn create_raydium_pool_ix(program_id: &Pubkey, pool: &Pubkey, params: &RaydiumPoolParams) -> Result<Instruction> {
    // Raydium pool initialization instruction data layout:
    // 0:   [u8; 8]  - instruction discriminator (createPool)
    // 8:   [u64]    - initial SOL amount
    // 16:  [u64]    - initial token amount
    // 24:  [u64]    - initial price
    // 32:  [u8; 32] - token mint pubkey
    
    let mut data = Vec::with_capacity(72);
    data.extend_from_slice(&[147, 216, 24, 218, 163, 45, 141, 86]); // "createPool" discriminator
    data.extend_from_slice(&params.sol_amount.to_le_bytes());
    data.extend_from_slice(&params.token_amount.to_le_bytes());
    data.extend_from_slice(&params.initial_price.to_le_bytes());
    data.extend_from_slice(&params.token_mint.to_bytes());

    Ok(Instruction {
        program_id: *program_id,
        accounts: vec![
            AccountMeta::new(*pool, false),
            AccountMeta::new_readonly(*program_id, false),
        ],
        data,
    })
}

fn create_raydium_swap_ix(
    program_id: &Pubkey,
    pool: &Pubkey,
    amount: u64,
    is_buy: bool,
) -> Result<Instruction> {
    // Raydium swap instruction data layout:
    // 0:   [u8; 8]  - instruction discriminator (swap)
    // 8:   [u64]    - amount
    // 16:  [bool]   - is_buy
    // 17:  [u8; 32] - pool pubkey
    
    let mut data = Vec::with_capacity(57);
    data.extend_from_slice(&[123, 98, 207, 75, 88, 145, 154, 23]); // "swap" discriminator
    data.extend_from_slice(&amount.to_le_bytes());
    data.push(is_buy as u8);
    data.extend_from_slice(&pool.to_bytes());

    Ok(Instruction {
        program_id: *program_id,
        accounts: vec![
            AccountMeta::new(*pool, false),
            AccountMeta::new_readonly(*program_id, false),
        ],
        data,
    })
}

// Helper functions for price estimation
fn estimate_received_tokens(sol_amount: u64) -> Result<u64> {
    // Using Raydium's constant product formula
    let tokens = sol_amount
        .checked_mul(997) // 0.3% fee
        .ok_or(ErrorCode::MathOverflow)?
        .checked_mul(1_000) // Price scale
        .ok_or(ErrorCode::MathOverflow)?
        .checked_div(1000)
        .ok_or(ErrorCode::MathOverflow)?;
    
    Ok(tokens)
}

fn estimate_received_sol(token_amount: u64) -> Result<u64> {
    // Using Raydium's constant product formula in reverse
    let sol = token_amount
        .checked_mul(997) // 0.3% fee
        .ok_or(ErrorCode::MathOverflow)?
        .checked_div(1_000) // Price scale
        .ok_or(ErrorCode::MathOverflow)?;
    
    Ok(sol)
}

// Price impact calculations for Raydium pool
fn calculate_price_impact(
    amount: u64,
    pool_token_amount: u64,
    pool_sol_amount: u64,
    is_buy: bool,
) -> Result<u64> {
    if is_buy {
        // For buys: (new_price - old_price) / old_price
        let old_price = pool_sol_amount
            .checked_div(pool_token_amount)
            .ok_or(ErrorCode::MathOverflow)?;
        
        let new_price = (pool_sol_amount + amount)
            .checked_div(pool_token_amount)
            .ok_or(ErrorCode::MathOverflow)?;
        
        let impact = ((new_price - old_price) * 10000) / old_price; // In basis points
        Ok(impact)
    } else {
        // For sells: (old_price - new_price) / old_price
        let old_price = pool_sol_amount
            .checked_div(pool_token_amount)
            .ok_or(ErrorCode::MathOverflow)?;
        
        let new_price = (pool_sol_amount - amount)
            .checked_div(pool_token_amount)
            .ok_or(ErrorCode::MathOverflow)?;
        
        let impact = ((old_price - new_price) * 10000) / old_price; // In basis points
        Ok(impact)
    }
}
