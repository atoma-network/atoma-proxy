ALTER TABLE users RENAME COLUMN username TO email;

ALTER TABLE users ADD COLUMN name TEXT;

UPDATE users SET name = email;
