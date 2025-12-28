export interface UpstreamProxyConfig {
    enabled: boolean;
    url: string;
}

export interface ProxyConfig {
    enabled: boolean;
    allow_lan_access?: boolean;
    port: number;
    api_key: string;
    auto_start: boolean;
    anthropic_mapping?: Record<string, string>;
    openai_mapping?: Record<string, string>;
    custom_mapping?: Record<string, string>;
    request_timeout: number;
    upstream_proxy: UpstreamProxyConfig;
}

export interface AppConfig {
    language: string;
    theme: string;
    auto_refresh: boolean;
    refresh_interval: number;
    auto_sync: boolean;
    sync_interval: number;
    default_export_path?: string;
    antigravity_executable?: string; // [NEW] Manually specified Antigravity executable path
    auto_launch?: boolean; // Auto launch on startup
    accounts_page_size?: number; // Number of accounts per page, default 0 means auto calculation
    proxy: ProxyConfig;
}

