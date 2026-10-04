-- Accounts opened before email login have neither value.
-- New accounts always set both. A unique index allows those empty rows.
ALTER TABLE users
    ADD COLUMN email text,
    ADD COLUMN password_hash text,
    ADD CONSTRAINT users_credentials_pair CHECK (
        (email IS NULL) = (password_hash IS NULL)
    );

CREATE UNIQUE INDEX users_email_key ON users (email);
