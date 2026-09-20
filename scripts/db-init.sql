-- Runs once when the docker volume is first created.
-- The application database itself is created by POSTGRES_DB; migrations run
-- automatically when gomoku-server starts.
--
-- Integration tests create and drop their own throwaway databases named
-- gomoku_test_*, which needs CREATEDB on the role.
ALTER ROLE gomoku CREATEDB;
