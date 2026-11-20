-- CRM rows the poll scenario links mail to (gen_crm_inbox_poll.py and crm_inbox_poll_golden.rs).
INSERT INTO organizations (id, name, website, is_deleted) VALUES
  ('00000000-0000-0000-0000-0000000005a1', 'PT Acme', 'https://www.acme.test', false);
INSERT INTO contacts (id, first_name, email, organization_id, is_deleted) VALUES
  ('00000000-0000-0000-0000-0000000005c1', 'Ann', 'Ann@Customer.test', '00000000-0000-0000-0000-0000000005a1', false);
INSERT INTO leads (id, source, contact_id, status, is_deleted, created_at) VALUES
  ('00000000-0000-0000-0000-0000000005b1', 'website', '00000000-0000-0000-0000-0000000005c1', 'qualifying', false, '2026-01-01T00:00:00Z');
INSERT INTO deals (id, title, contact_id, stage, is_deleted, created_at) VALUES
  ('00000000-0000-0000-0000-0000000005d1', 'Deal', '00000000-0000-0000-0000-0000000005c1', 'proposal', false, '2026-01-01T00:00:00Z'),
  ('00000000-0000-0000-0000-0000000005d2', 'Won deal', '00000000-0000-0000-0000-0000000005c1', 'won', false, '2026-02-01T00:00:00Z');
UPDATE system_settings SET value = 'false' WHERE key = 'nary.email.intake_enabled';
