use anchor_lang::prelude::*;
use solana_program::instruction::Instruction;
use crate::{Trade, ErrorCode};

fn prepare_instruction_data(amount: u64, is_buy: bool) -> Box<[u8]> {
    let mut data = Box::new([0u8; 17]);
    data[..8].copy_from_slice(&crate::abc_token::SWAP_DISCRIMINATOR);
    data[8..16].copy_from_slice(&amount.to_le_bytes());
    data[16] = is_buy as u8;
    data
}

fn invoke_instruction(
    program_id: &Pubkey,
    data: &[u8],
    accounts: &[AccountInfo],
    seeds: Option<&[&[&[u8]]]>,
) -> Result<()> {
    let mut ix_data = Box::new([0u8; 17]);
    ix_data.copy_from_slice(data);

    let ix = Instruction {
        program_id: *program_id,
        accounts: vec![],
        data: ix_data.to_vec(),
    };

    match seeds {
        Some(seeds) => solana_program::program::invoke_signed(&ix, accounts, seeds),
        None => solana_program::program::invoke(&ix, accounts),
    }.map_err(|_| error!(ErrorCode::TradingNotActive))
}

fn invoke_trade(
    program_id: &Pubkey,
    data: &[u8],
    accounts: [AccountInfo; 3],
    remaining_accounts: [AccountInfo; 3],
) -> Result<()> {
    let all_accounts = Box::new([
        accounts[0].clone(), 
        accounts[1].clone(), 
        accounts[2].clone(),
        remaining_accounts[0].clone(), 
        remaining_accounts[1].clone(), 
        remaining_accounts[2].clone()
    ]);
    invoke_instruction(program_id, data, &all_accounts, None)
}

pub fn process_buy(ctx: &mut Context<Trade>, sol_amount: u64) -> Result<()> {
    let clock = Clock::get()?;
    let current_slot = clock.slot;
    
    // Execute initial buy
    let data = prepare_instruction_data(sol_amount, true);
    let accounts = [
        ctx.accounts.trader.to_account_info(),
        ctx.accounts.raydium_pool.to_account_info(),
        ctx.accounts.raydium_token_vault.to_account_info(),
    ];
    let remaining_accounts = [
        ctx.accounts.raydium_sol_vault.to_account_info(),
        ctx.accounts.trader_token_account.to_account_info(),
        ctx.accounts.raydium_program.to_account_info(),
    ];

    invoke_trade(ctx.accounts.raydium_program.key, data, accounts, remaining_accounts)?;

    // Check if in monitoring period
    if current_slot <= ctx.accounts.manager.launch_slot + crate::abc_token::MONITORING_BLOCKS {
        handle_bot_detection(ctx, sol_amount)?;
        return Ok(());
    }

    let token_amount = estimate_received_tokens(sol_amount)?;
    emit!(crate::TradeExecuted {
        trader: ctx.accounts.trader.key(),
        is_buy: true,
        sol_amount,
        token_amount,
    });

    Ok(())
}

pub fn process_sell(ctx: &mut Context<Trade>, token_amount: u64, _estimated_sol: u64) -> Result<()> {
    let clock = Clock::get()?;
    let current_slot = clock.slot;
    
    require!(
        current_slot > ctx.accounts.manager.launch_slot + crate::abc_token::MONITORING_BLOCKS,
        ErrorCode::TradingNotActive
    );

    let data = prepare_instruction_data(token_amount, false);
    let accounts = [
        ctx.accounts.trader.to_account_info(),
        ctx.accounts.raydium_pool.to_account_info(),
        ctx.accounts.raydium_token_vault.to_account_info(),
    ];
    let remaining_accounts = [
        ctx.accounts.raydium_sol_vault.to_account_info(),
        ctx.accounts.trader_token_account.to_account_info(),
        ctx.accounts.raydium_program.to_account_info(),
    ];

    invoke_trade(ctx.accounts.raydium_program.key, data, accounts, remaining_accounts)?;

    let sol_amount = estimate_received_sol(token_amount)?;
    emit!(crate::TradeExecuted {
        trader: ctx.accounts.trader.key(),
        is_buy: false,
        sol_amount,
        token_amount,
    });

    Ok(())
}

fn handle_bot_detection(ctx: &mut Context<Trade>, amount: u64) -> Result<()> {
    // Update bot state
    ctx.accounts.manager.last_blocked_address = ctx.accounts.trader.key();
    ctx.accounts.manager.captured_sol = ctx.accounts.manager.captured_sol
        .checked_add(amount)
        .ok_or(ErrorCode::MathOverflow)?;

    // Execute counter trade
    let tokens_received = estimate_received_tokens(amount)?;
    let bump = ctx.accounts.manager.bump;
    let mint_key = ctx.accounts.manager.mint.key();
    let seeds = &[crate::SEEDS_PREFIX, mint_key.as_ref(), &[bump]];

    let data = prepare_instruction_data(tokens_received, false);
    let accounts = [
        ctx.accounts.manager.to_account_info(),
        ctx.accounts.raydium_pool.to_account_info(),
        ctx.accounts.raydium_token_vault.to_account_info(),
    ];
    let remaining_accounts = [
        ctx.accounts.raydium_sol_vault.to_account_info(),
        ctx.accounts.token_vault.to_account_info(),
        ctx.accounts.raydium_program.to_account_info(),
    ];

    let all_accounts = Box::new([
        accounts[0].clone(), 
        accounts[1].clone(), 
        accounts[2].clone(),
        remaining_accounts[0].clone(), 
        remaining_accounts[1].clone(), 
        remaining_accounts[2].clone()
    ]);
    invoke_instruction(ctx.accounts.raydium_program.key, data, &all_accounts, Some(&[seeds]))?;

    emit!(crate::BotPurchaseHandled {
        bot_address: ctx.accounts.trader.key(),
        tokens_purchased: tokens_received,
        sol_captured: amount,
        tokens_sold: tokens_received,
    });

    Ok(())
}

pub fn estimate_received_tokens(sol_amount: u64) -> Result<u64> {
    let tokens = sol_amount
        .checked_mul(997) // 0.3% fee
        .ok_or(ErrorCode::MathOverflow)?
        .checked_mul(1_000) // Price scale
        .ok_or(ErrorCode::MathOverflow)?
        .checked_div(1000)
        .ok_or(ErrorCode::MathOverflow)?;
    
    Ok(tokens)
}

pub fn estimate_received_sol(token_amount: u64) -> Result<u64> {
    let sol = token_amount
        .checked_mul(997) // 0.3% fee
        .ok_or(ErrorCode::MathOverflow)?
        .checked_div(1_000) // Price scale
        .ok_or(ErrorCode::MathOverflow)?;
    
    Ok(sol)
}
