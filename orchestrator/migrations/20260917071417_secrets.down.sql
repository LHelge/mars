-- Reverses the `.up.sql` exactly: its tables in reverse order, the audit table
-- first because it references `secrets`. The indexes belong to their tables
-- and are dropped with them.

DROP TABLE secret_uses;

DROP TABLE secrets;
