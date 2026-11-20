-- Fixed CRM data for the crm golden fixtures (gen_crm.py runs it in a scratch database built by
-- migrations + seed; the Rust tests run it in support::fresh_db). Ids are fixed so results compare.
INSERT INTO users (id, email, name, password_hash, is_active, is_superadmin, is_system_admin, portal_type, created_at, updated_at) VALUES
 ('00000000-0000-0000-0000-0000000000a0', 'boss@s15.test', 'Boss', 'x', true, true, false, 'internal', now(), now()),
 ('00000000-0000-0000-0000-0000000000a1', 'sales1@s15.test', 'Sales One', 'x', true, false, false, 'internal', now(), now()),
 ('00000000-0000-0000-0000-0000000000a2', 'Sales2@S15.test', 'Sales Two', 'x', true, false, false, 'internal', now(), now()),
 ('00000000-0000-0000-0000-0000000000a3', 'gone@s15.test', 'Gone', 'x', false, false, false, 'internal', now(), now());

INSERT INTO crm_attribute_defs (id, object_type, key, label, field_type, options, is_ai, sort_order, is_active, created_at, updated_at) VALUES
 ('00000000-0000-0000-0000-00000000d001', 'organization', 'tier', 'Tier', 'select', '{"choices": ["Tier 1", "Tier 2"]}', false, 1, true, '2026-01-01 00:00:01+00', now()),
 ('00000000-0000-0000-0000-00000000d002', 'organization', 'score', 'Score', 'number', '{}', false, 2, true, '2026-01-01 00:00:02+00', now()),
 ('00000000-0000-0000-0000-00000000d003', 'organization', 'tags', 'Tags', 'multi_select', '{"choices": ["South", "North"]}', false, 3, true, '2026-01-01 00:00:03+00', now()),
 ('00000000-0000-0000-0000-00000000d004', 'organization', 'flag', 'Flag', 'checkbox', null, false, 4, true, '2026-01-01 00:00:04+00', now()),
 ('00000000-0000-0000-0000-00000000d005', 'organization', 'when', 'When', 'date', '{}', false, 5, true, '2026-01-01 00:00:05+00', now()),
 ('00000000-0000-0000-0000-00000000d006', 'organization', 'city', 'City (custom)', 'text', '{}', false, 0, true, '2026-01-01 00:00:06+00', now()),
 ('00000000-0000-0000-0000-00000000d007', 'organization', 'old', 'Old', 'text', '{}', false, 0, false, '2026-01-01 00:00:07+00', now()),
 ('00000000-0000-0000-0000-00000000d008', 'deal', 'segment', 'Segment', 'select', '{"choices": ["SMB", "Enterprise"]}', false, 1, true, '2026-01-01 00:00:08+00', now()),
 ('00000000-0000-0000-0000-00000000d009', 'contact', 'linkedin', 'LinkedIn', 'url', '{}', false, 1, true, '2026-01-01 00:00:09+00', now());

INSERT INTO organizations (id, name, industry, country, city, website, email, employee_count, annual_revenue, assigned_to_id, custom, is_deleted, created_at, updated_at) VALUES
 ('00000000-0000-0000-0000-000000000101', 'Globex Corp', 'Manufacturing', 'Testland', 'Springfield', 'https://globex.example', 'info@globex.example', 120, 1500000.50, '00000000-0000-0000-0000-0000000000a1', '{"tier": "Tier 1", "score": "12.5", "tags": ["South"], "flag": true, "when": "2026-03-01", "city": "SBY"}', false, '2026-02-01 08:00:00+00', now()),
 ('00000000-0000-0000-0000-000000000102', 'Initech 50% Off_Co', 'manufacturing', 'Testland', 'Shelbyville', null, null, 5, 0.10, '00000000-0000-0000-0000-0000000000a2', '{"tier": "Tier 2", "score": "", "tags": ["North", "South"]}', false, '2026-02-02 08:00:00+00', now()),
 ('00000000-0000-0000-0000-000000000103', 'Acme Holdings', 'Retail', 'Singapore', null, null, 'ops@acme.example', null, 1.01, null, '{"score": "7"}', false, '2026-02-03 08:00:00+00', now()),
 ('00000000-0000-0000-0000-000000000104', 'Deleted Org', 'Manufacturing', 'Testland', 'Springfield', null, null, 10, null, null, '{}', true, '2026-02-04 08:00:00+00', now()),
 ('00000000-0000-0000-0000-000000000105', 'Ünïcode Örg', 'MANUFACTURING', 'Malaysia', 'Capital City', null, null, 6, 2.00, '00000000-0000-0000-0000-0000000000a1', '{"tier": "tier 1", "score": "100.004"}', false, '2026-02-05 08:00:00+00', now()),
 ('00000000-0000-0000-0000-000000000106', 'Same Time A', 'Logistics', 'Testland', 'Ogdenville', null, null, 6, 1.00, null, '{}', false, '2026-02-06 08:00:00+00', now()),
 ('00000000-0000-0000-0000-000000000107', 'Same Time B', 'Logistics', 'Testland', 'Ogdenville', null, null, 7, 1.00, null, '{}', false, '2026-02-06 08:00:00+00', now());

INSERT INTO contacts (id, first_name, last_name, email, position, department, country, organization_id, assigned_to_id, custom, is_deleted, created_at, updated_at) VALUES
 ('00000000-0000-0000-0000-000000000201', 'John', 'Scott', 'john@globex.example', 'CTO', 'IT', 'Testland', '00000000-0000-0000-0000-000000000101', '00000000-0000-0000-0000-0000000000a1', '{}', false, '2026-03-01 08:00:00+00', now()),
 ('00000000-0000-0000-0000-000000000202', 'Mary', null, null, 'Buyer', null, 'Testland', '00000000-0000-0000-0000-000000000101', null, '{"linkedin": "https://linkedin.example/mary"}', false, '2026-03-02 08:00:00+00', now()),
 ('00000000-0000-0000-0000-000000000203', ' Lone ', ' ', null, null, null, null, null, '00000000-0000-0000-0000-0000000000a2', '{}', false, '2026-03-03 08:00:00+00', now()),
 ('00000000-0000-0000-0000-000000000204', 'Gone', 'Contact', null, null, null, null, '00000000-0000-0000-0000-000000000101', null, '{}', true, '2026-03-04 08:00:00+00', now());

INSERT INTO leads (id, title, status, priority, source, source_detail, estimated_value, currency, interested_products, organization_id, contact_id, assigned_to_id, custom, is_deleted, created_at, updated_at) VALUES
 ('00000000-0000-0000-0000-000000000301', 'Facility laptops for 12 sites', 'new', 'high', 'web3forms', 'homepage', 25000.00, 'USD', '{laptops,support}', '00000000-0000-0000-0000-000000000101', '00000000-0000-0000-0000-000000000201', '00000000-0000-0000-0000-0000000000a1', '{}', false, '2026-04-01 08:00:00+00', now()),
 ('00000000-0000-0000-0000-000000000302', null, 'qualifying', null, 'email', null, null, null, null, null, null, null, '{}', false, '2026-04-02 08:00:00+00', now()),
 ('00000000-0000-0000-0000-000000000303', 'Spam lead', 'disqualified', 'low', 'other', null, null, 'JPY', null, null, null, '00000000-0000-0000-0000-0000000000a2', '{}', false, '2026-04-03 08:00:00+00', now()),
 ('00000000-0000-0000-0000-000000000304', 'Acme Holdings traffic', 'triaging', 'medium', 'linkedin', '', 0.00, 'SGD', '{}', '00000000-0000-0000-0000-000000000103', null, '00000000-0000-0000-0000-0000000000a2', '{}', false, '2026-04-04 08:00:00+00', now());

INSERT INTO deals (id, title, stage, value, currency, probability, expected_close_date, organization_id, contact_id, lead_id, assigned_to_id, product_interests, product_line_ids, custom, is_deleted, created_at, updated_at) VALUES
 ('00000000-0000-0000-0000-000000000401', 'Globex rollout', 'proposal', 50000.00, 'USD', 50, '2026-12-01', '00000000-0000-0000-0000-000000000101', '00000000-0000-0000-0000-000000000201', null, '00000000-0000-0000-0000-0000000000a1', '{}', '{}', '{"segment": "Enterprise"}', false, '2026-05-01 08:00:00+00', now()),
 ('00000000-0000-0000-0000-000000000402', 'Globex pilot', 'won', 0.00, 'JPY', 100, '2026-06-01', '00000000-0000-0000-0000-000000000101', null, null, '00000000-0000-0000-0000-0000000000a2', '{}', '{}', '{}', false, '2026-05-02 08:00:00+00', now()),
 ('00000000-0000-0000-0000-000000000403', 'Acme Holdings', 'lost', 1234.56, null, 0, null, '00000000-0000-0000-0000-000000000103', null, null, null, '{}', '{}', '{}', false, '2026-05-03 08:00:00+00', now()),
 ('00000000-0000-0000-0000-000000000404', 'Old deal', 'new', 99.99, 'USD', 5, '2025-01-01', '00000000-0000-0000-0000-000000000101', null, null, null, '{}', '{}', '{}', true, '2026-05-04 08:00:00+00', now());
UPDATE deals SET loss_reasons = '["price_too_expensive"]', loss_reason = 'price_too_expensive', actual_close_date = '2026-05-10' WHERE id = '00000000-0000-0000-0000-000000000403';

INSERT INTO activities (id, entity_type, entity_id, contact_id, lead_id, deal_id, activity_type, direction, subject, content, outcome, scheduled_at, completed_at, created_at, updated_at) VALUES
 ('00000000-0000-0000-0000-000000000501', 'deal', '00000000-0000-0000-0000-000000000401', '00000000-0000-0000-0000-000000000201', null, '00000000-0000-0000-0000-000000000401', 'call', 'outbound', 'Discovery', 'Talked about 12 sites.', 'positive', null, '2026-06-01 09:30:00.123456+00', '2026-06-01 09:00:00+00', now()),
 ('00000000-0000-0000-0000-000000000502', 'organization', '00000000-0000-0000-0000-000000000101', null, null, null, 'site_visit', 'internal', null, null, null, '2026-06-03 02:00:00+00', null, '2026-06-02 09:00:00+00', now()),
 ('00000000-0000-0000-0000-000000000503', 'lead', '00000000-0000-0000-0000-000000000301', null, '00000000-0000-0000-0000-000000000301', null, 'email', 'inbound', '', 'Long content Ünïcode ' || repeat('é', 300), null, null, null, '2026-06-02 09:00:00+00', now()),
 ('00000000-0000-0000-0000-000000000504', 'contact', '00000000-0000-0000-0000-000000000202', '00000000-0000-0000-0000-000000000202', null, null, 'whatsapp', 'inbound', 'Hi', '', null, null, null, '2026-06-01 09:00:00+00', now());

INSERT INTO crm_email_threads (id, subject, status, message_count, last_sender_email, last_sender_name, last_message_at, linked_org_id, linked_contact_id, linked_lead_id, linked_deal_id, created_at, updated_at) VALUES
 ('00000000-0000-0000-0000-000000000601', 'Quote follow-up', 'open', 3, 'john@globex.example', 'John', '2026-06-01 09:30:00+00', '00000000-0000-0000-0000-000000000101', null, null, null, '2026-05-30 00:00:00+00', now()),
 ('00000000-0000-0000-0000-000000000602', null, 'resolved', null, null, null, null, null, '00000000-0000-0000-0000-000000000201', null, null, '2026-05-31 00:00:00+00', now()),
 ('00000000-0000-0000-0000-000000000603', '', 'in_progress', 1, 'x@y.example', '', '2026-06-05 00:00:00+00', null, null, '00000000-0000-0000-0000-000000000301', null, '2026-06-04 00:00:00+00', now());

INSERT INTO quotations (id, number, full_number, status, currency, total, issue_date, valid_until, deal_id, customer_org_id, customer_contact_id, is_deleted, created_at, updated_at) VALUES
 ('00000000-0000-0000-0000-000000000701', 'Q-1', 'Q-2026-0001', 'sent', 'USD', 1234567.89, '2026-06-01', '2026-07-01', '00000000-0000-0000-0000-000000000401', '00000000-0000-0000-0000-000000000101', null, false, '2026-06-01 01:00:00+00', now()),
 ('00000000-0000-0000-0000-000000000702', 'Q-2', null, 'draft', null, null, '2026-06-02', '2026-07-01', null, null, '00000000-0000-0000-0000-000000000201', false, '2026-06-02 01:00:00+00', now()),
 ('00000000-0000-0000-0000-000000000703', 'Q-3', 'Q-2026-0003', 'sent', 'JPY', 2.5, '2026-06-03', '2026-07-01', null, '00000000-0000-0000-0000-000000000101', null, true, '2026-06-03 01:00:00+00', now());

INSERT INTO invoices (id, number, full_number, status, currency, total, issue_date, deal_id, customer_org_id, customer_contact_id, is_deleted, created_at, updated_at) VALUES
 ('00000000-0000-0000-0000-000000000801', 'I-1', 'INV-2026-0001', 'paid', 'USD', 0.5, '2026-06-01', '00000000-0000-0000-0000-000000000401', null, null, false, '2026-06-01 02:00:00+00', now()),
 ('00000000-0000-0000-0000-000000000802', 'I-2', '', 'draft', 'JPY', 1500.5, '2026-05-15', null, '00000000-0000-0000-0000-000000000101', null, false, '2026-05-15 02:00:00+00', now());
