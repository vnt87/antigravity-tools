import { invoke } from '@tauri-apps/api/core';
import { Account, QuotaData } from '../types/account';

// Check Tauri environment
function ensureTauriEnvironment() {
    // Only check if invoke function is available
    // Do not check __TAURI__ object, as it may not exist in some Tauri versions
    if (typeof invoke !== 'function') {
        throw new Error('Tauri API not loaded correctly, please restart the app');
    }
}

export async function listAccounts(): Promise<Account[]> {
    return await invoke('list_accounts');
}

export async function getCurrentAccount(): Promise<Account | null> {
    return await invoke('get_current_account');
}

export async function addAccount(email: string, refreshToken: string): Promise<Account> {
    return await invoke('add_account', { email, refreshToken });
}

export async function deleteAccount(accountId: string): Promise<void> {
    return await invoke('delete_account', { accountId });
}

export async function deleteAccounts(accountIds: string[]): Promise<void> {
    return await invoke('delete_accounts', { accountIds });
}

export async function switchAccount(accountId: string): Promise<void> {
    return await invoke('switch_account', { accountId });
}

export async function fetchAccountQuota(accountId: string): Promise<QuotaData> {
    return await invoke('fetch_account_quota', { accountId });
}

export interface RefreshStats {
    total: number;
    success: number;
    failed: number;
    details: string[];
}

export async function refreshAllQuotas(): Promise<RefreshStats> {
    return await invoke('refresh_all_quotas');
}

// OAuth
export async function startOAuthLogin(): Promise<Account> {
    ensureTauriEnvironment();

    try {
        return await invoke('start_oauth_login');
    } catch (error) {
        // Enhance error message
        if (typeof error === 'string') {
            // If it is a missing refresh_token error, keep it as is (already contains detailed explanation)
            if (error.includes('Refresh Token') || error.includes('refresh_token')) {
                throw error;
            }
            // Add context for other errors
            throw `OAuth authorization failed: ${error}`;
        }
        throw error;
    }
}

export async function completeOAuthLogin(): Promise<Account> {
    ensureTauriEnvironment();
    try {
        return await invoke('complete_oauth_login');
    } catch (error) {
        if (typeof error === 'string') {
            if (error.includes('Refresh Token') || error.includes('refresh_token')) {
                throw error;
            }
            throw `OAuth authorization failed: ${error}`;
        }
        throw error;
    }
}

export async function cancelOAuthLogin(): Promise<void> {
    ensureTauriEnvironment();
    return await invoke('cancel_oauth_login');
}

// Import
export async function importV1Accounts(): Promise<Account[]> {
    return await invoke('import_v1_accounts');
}

export async function importFromDb(): Promise<Account> {
    return await invoke('import_from_db');
}

export async function importFromCustomDb(path: string): Promise<Account> {
    return await invoke('import_custom_db', { path });
}

export async function syncAccountFromDb(): Promise<Account | null> {
    return await invoke('sync_account_from_db');
}
