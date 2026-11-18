alter table provider_accounts
    add column display_name text check (octet_length(display_name) between 1 and 512);
