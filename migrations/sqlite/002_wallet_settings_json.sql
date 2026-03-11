-- Add walletSettingsJson column to settings table for wallet settings persistence.
-- This allows WalletSettingsManager to store settings JSON directly in the
-- settings table without requiring the LocalKVStore/createAction pattern.
ALTER TABLE settings ADD COLUMN walletSettingsJson TEXT;
