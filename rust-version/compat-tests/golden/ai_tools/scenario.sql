-- The scenario the ai/tools golden calls run against (gen_ai_tools_calls.py and tests/ai_tools_golden.rs).
-- Fixed ids and timestamps; {{NOW-<n>d}} / {{NOW-<n>h}} are replaced by the runner with the current hour
-- (UTC) minus that much, and the same text in a tool's output is mapped back to the token.

-- users (permissions are given per call, not through roles)
INSERT INTO users (id, email, name, password_hash, is_active, is_superadmin, is_system_admin, portal_type) VALUES
 ('a0000000-0000-0000-0000-000000000001', 'golden.owner@example.com', 'Owner Golden', 'x', true, true, false, 'internal'),
 ('a0000000-0000-0000-0000-000000000002', 'rachel@example.com', 'Rachel Sales', 'x', true, false, false, 'internal'),
 ('a0000000-0000-0000-0000-000000000003', 'john@example.com', 'John Ops', 'x', true, false, false, 'internal'),
 ('a0000000-0000-0000-0000-000000000004', 'debbie@example.com', 'Debbie Lead', 'x', true, false, false, 'internal'),
 ('a0000000-0000-0000-0000-000000000005', 'gus.s@example.com', 'Adam Scott', 'x', true, false, false, 'internal'),
 ('a0000000-0000-0000-0000-000000000006', 'gus.w@example.com', 'Gus Wilson', 'x', true, false, false, 'internal'),
 ('a0000000-0000-0000-0000-000000000007', 'gone@example.com', 'Gone Person', 'x', false, false, false, 'internal');

-- organizations
INSERT INTO organizations (id, name, city, industry, assigned_to_id, is_deleted, created_at) VALUES
 ('b0000000-0000-0000-0000-000000000001', 'Globex Corp', 'Springfield', 'manufacturing', 'a0000000-0000-0000-0000-000000000002', false, '2026-01-01 01:00:00+00'),
 ('b0000000-0000-0000-0000-000000000002', 'Globex Steel', 'Tokyo', 'manufacturing', 'a0000000-0000-0000-0000-000000000003', false, '2026-01-01 02:00:00+00'),
 ('b0000000-0000-0000-0000-000000000003', 'Initech East', 'Kyoto', 'retail', 'a0000000-0000-0000-0000-000000000002', false, '2026-01-01 03:00:00+00'),
 ('b0000000-0000-0000-0000-000000000004', 'Initech West', 'Kyoto', 'retail', 'a0000000-0000-0000-0000-000000000002', false, '2026-01-01 04:00:00+00'),
 ('b0000000-0000-0000-0000-000000000005', 'Deleted Corp Corp', 'Kyoto', 'retail', NULL, true, '2026-01-01 05:00:00+00');

-- contacts
INSERT INTO contacts (id, first_name, last_name, email, phone, organization_id, assigned_to_id, is_deleted, created_at) VALUES
 ('c0000000-0000-0000-0000-000000000001', 'Diana', 'Prince', 'diana@globex.test', '+1555010001111', 'b0000000-0000-0000-0000-000000000001', 'a0000000-0000-0000-0000-000000000002', false, '2026-01-02 01:00:00+00'),
 ('c0000000-0000-0000-0000-000000000002', 'John', 'Smith', 'john.h@steel.test', '+1555010002222', 'b0000000-0000-0000-0000-000000000002', 'a0000000-0000-0000-0000-000000000003', false, '2026-01-02 02:00:00+00'),
 ('c0000000-0000-0000-0000-000000000003', 'Diana', NULL, NULL, '+1555010001111', NULL, 'a0000000-0000-0000-0000-000000000002', false, '2026-01-02 03:00:00+00'),
 ('c0000000-0000-0000-0000-000000000004', 'Mary', '  ', 'mary@globex.test', NULL, 'b0000000-0000-0000-0000-000000000001', NULL, false, '2026-01-02 04:00:00+00'),
 ('c0000000-0000-0000-0000-000000000005', 'Hidden', 'Person', 'hidden@globex.test', NULL, NULL, NULL, true, '2026-01-02 05:00:00+00');

-- leads
INSERT INTO leads (id, title, status, priority, source, source_detail, estimated_value, currency, description, contact_id, organization_id, assigned_to_id, is_deleted, created_at) VALUES
 ('d0000000-0000-0000-0000-000000000001', 'Golden: SDK inquiry', 'new', 'high', 'email', 'hello@ inbox', 15000000.50, 'JPY', 'Wants laptops for 3 offices. Globex Corp plant in Shelbyville.', 'c0000000-0000-0000-0000-000000000001', 'b0000000-0000-0000-0000-000000000001', 'a0000000-0000-0000-0000-000000000002', false, '2026-01-03 01:00:00+00'),
 ('d0000000-0000-0000-0000-000000000002', 'Steel yard safety', 'qualifying', 'medium', 'website', NULL, NULL, NULL, NULL, 'c0000000-0000-0000-0000-000000000002', 'b0000000-0000-0000-0000-000000000002', 'a0000000-0000-0000-0000-000000000003', false, '2026-01-03 02:00:00+00'),
 ('d0000000-0000-0000-0000-000000000003', 'Orphan lead', 'new', NULL, 'referral', NULL, NULL, NULL, 'No contact, no company', NULL, NULL, NULL, false, '2026-01-03 03:00:00+00');

-- deals
INSERT INTO deals (id, title, deal_number, stage, value, currency, probability, expected_close_date, contact_id, lead_id, organization_id, assigned_to_id, is_deleted, created_at) VALUES
 ('e0000000-0000-0000-0000-000000000001', 'Golden gates rollout', 'DL-0001', 'proposal', 250000000.00, 'JPY', 50, '2026-12-01', 'c0000000-0000-0000-0000-000000000001', 'd0000000-0000-0000-0000-000000000001', 'b0000000-0000-0000-0000-000000000001', 'a0000000-0000-0000-0000-000000000002', false, '2026-01-04 01:00:00+00'),
 ('e0000000-0000-0000-0000-000000000002', 'Steel yard pilot', 'DL-0002', 'won', 1000.50, 'USD', 100, NULL, 'c0000000-0000-0000-0000-000000000002', NULL, 'b0000000-0000-0000-0000-000000000002', 'a0000000-0000-0000-0000-000000000003', false, '2026-01-04 02:00:00+00'),
 ('e0000000-0000-0000-0000-000000000003', 'Legacy stage deal', NULL, 'legacy_x', NULL, NULL, NULL, NULL, NULL, NULL, NULL, 'a0000000-0000-0000-0000-000000000002', false, '2026-01-04 03:00:00+00'),
 ('e0000000-0000-0000-0000-000000000004', 'Second proposal', 'DL-0004', 'proposal', 0.10, 'JPY', 40, NULL, NULL, NULL, NULL, 'a0000000-0000-0000-0000-000000000003', false, '2026-01-04 04:00:00+00'),
 ('e0000000-0000-0000-0000-000000000005', 'Deleted deal', NULL, 'new', 99.00, 'JPY', NULL, NULL, NULL, NULL, NULL, NULL, true, '2026-01-04 05:00:00+00');

-- invoices
INSERT INTO invoices (id, number, issue_date, customer_name, status, total, amount_paid, amount_due, currency, due_date, is_deleted) VALUES
 ('f0000000-0000-0000-0000-000000000001', 'INV-2020-001', '2020-01-01', 'Globex Corp', 'sent', 2000000.00, 500000.00, 1500000.00, 'JPY', '2020-01-15', false),
 ('f0000000-0000-0000-0000-000000000002', 'INV-2020-002', '2020-01-01', 'Globex Steel', 'paid', 300.25, 300.25, 0, 'USD', '2020-01-10', false),
 ('f0000000-0000-0000-0000-000000000003', 'INV-2020-003', '2020-01-01', 'Draft Co', 'draft', 100.00, 0, 100.00, 'JPY', '2020-01-05', false),
 ('f0000000-0000-0000-0000-000000000004', 'INV-2020-004', '2020-01-01', NULL, 'partially_paid', 1234.56, 234.56, 1000.00, 'USD', '2020-02-01', false),
 ('f0000000-0000-0000-0000-000000000005', 'INV-2099-005', '2026-01-01', 'Future Co', 'sent', 10.00, 0, 10.00, 'JPY', '2099-01-01', false),
 ('f0000000-0000-0000-0000-000000000006', 'INV-2020-006', '2020-01-01', 'Gone Co', 'sent', 77.00, 0, 77.00, 'JPY', '2020-01-01', true);

-- projects
INSERT INTO projects (id, name, project_number, status, project_type, budget, currency, cost_to_date, start_date, end_date, description, pm_id, tech_lead_id, created_by_id, is_deleted, created_at) VALUES
 ('10000000-0000-0000-0000-000000000001', 'Golden Gates Deployment', 'PRJ-001', 'active', 'deployment', 150000000.00, 'JPY', 2500.75, '2026-02-01', '2026-06-30', 'Install 3 gate readers.', 'a0000000-0000-0000-0000-000000000002', NULL, 'a0000000-0000-0000-0000-000000000001', false, '2026-01-05 01:00:00+00'),
 ('10000000-0000-0000-0000-000000000002', 'Steel Yard Pilot', 'PRJ-002', 'planning', 'pilot', NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 'a0000000-0000-0000-0000-000000000001', false, '2026-01-05 02:00:00+00'),
 ('10000000-0000-0000-0000-000000000003', 'Internal R&D', 'PRJ-003', 'active', 'internal', NULL, NULL, NULL, NULL, NULL, NULL, NULL, 'a0000000-0000-0000-0000-000000000004', 'a0000000-0000-0000-0000-000000000001', false, '2026-01-05 03:00:00+00');
INSERT INTO project_members (id, project_id, user_id) VALUES
 ('11000000-0000-0000-0000-000000000001', '10000000-0000-0000-0000-000000000002', 'a0000000-0000-0000-0000-000000000003');

-- catalog (every SKU carries ZQX so the searches only see these)
INSERT INTO product_catalog (id, sku, name, short_name, description, category, family, product_type, unit, sale_model, price_metric, pricing_mode, percent_rate, min_quantity, min_term_months, modules, addon_for, included_support_months, is_whitelabel, whitelabel_free_min_qty, whitelabel_rule, terms_note, cost_price, is_active, is_orderable, is_deleted) VALUES
 ('20000000-0000-0000-0000-000000000001', 'ZQX-BOX-01', 'ZQX Laptop & Docking Kit', 'ZQX Box', 'Laptop with a docking station', 'hardware', 'zqx_box', 'hardware', 'unit', 'one_time', 'unit', 'list', NULL, 1.00, NULL, '{backup,sync}', NULL, 12, false, NULL, NULL, 'Ships in 2 weeks', 9000000.00, true, true, false), ('20000000-0000-0000-0000-000000000002', 'ZQX-QUOTE-02', 'ZQX Custom Integration', NULL, NULL, 'service', 'zqx_svc', 'service', 'project', 'one_time', 'unit', 'quote_only', NULL, NULL, NULL, NULL, NULL, NULL, false, NULL, NULL, NULL, NULL, true, true, false), ('20000000-0000-0000-0000-000000000003', 'ZQX-MAN-03', 'ZQX Manual Item', NULL, NULL, 'service', 'zqx_svc', 'service', NULL, 'one_time', 'unit', 'manual', NULL, NULL, NULL, NULL, NULL, NULL, false, NULL, NULL, NULL, NULL, true, true, false), ('20000000-0000-0000-0000-000000000004', 'ZQX-CARE-04', 'ZQX Support', NULL, NULL, 'support', 'zqx_support', 'service', 'year', 'recurring', 'unit', 'percent_of_parent', 15.00, NULL, 12, NULL, '{ZQX-BOX-01}', NULL, false, NULL, NULL, NULL, NULL, true, true, false), ('20000000-0000-0000-0000-000000000005', 'ZQX-WL-05', 'ZQX White Label Branding', NULL, NULL, 'branding', 'zqx_wl', 'service', NULL, 'one_time', 'unit', 'list', NULL, NULL, NULL, NULL, NULL, NULL, true, 50, '{"free_from": 50}', NULL, NULL, true, true, false), ('20000000-0000-0000-0000-000000000006', 'ZQX-SUB-06', 'ZQX Cloud Subscription', NULL, 'Monthly cloud backup', 'software', 'zqx_box', 'software', 'device', 'recurring', 'device', 'list', NULL, 4.00, 12, '{reporting}', NULL, NULL, false, NULL, NULL, NULL, 120000.00, true, true, false), ('20000000-0000-0000-0000-000000000007', 'ZQX-OFF-07', 'ZQX Retired Box', NULL, NULL, 'hardware', 'zqx_box', 'hardware', NULL, 'one_time', 'unit', 'list', NULL, NULL, NULL, NULL, NULL, NULL, false, NULL, NULL, NULL, NULL, false, true, false), ('20000000-0000-0000-0000-000000000008', 'ZQX-NOP-08', 'ZQX No Price', NULL, NULL, 'hardware', 'zqx_box', 'hardware', NULL, 'one_time', 'unit', 'list', NULL, NULL, NULL, NULL, NULL, NULL, false, NULL, NULL, NULL, NULL, true, true, false);
INSERT INTO catalog_prices (id, product_id, currency, amount, valid_from) VALUES
 ('21000000-0000-0000-0000-000000000001', '20000000-0000-0000-0000-000000000001', 'JPY', 12500000.00, '2020-01-01'),
 ('21000000-0000-0000-0000-000000000002', '20000000-0000-0000-0000-000000000001', 'USD', 799.99, '2020-01-01'),
 ('21000000-0000-0000-0000-000000000003', '20000000-0000-0000-0000-000000000006', 'JPY', 150000.00, '2020-01-01'),
 ('21000000-0000-0000-0000-000000000004', '20000000-0000-0000-0000-000000000005', 'JPY', 5000000.00, '2020-01-01');
INSERT INTO catalog_volume_tiers (id, product_id, min_qty, max_qty, discount_pct) VALUES
 ('22000000-0000-0000-0000-000000000002', '20000000-0000-0000-0000-000000000001', 10, NULL, 7.5),
 ('22000000-0000-0000-0000-000000000001', '20000000-0000-0000-0000-000000000001', 5, 9, 5);

-- personal mailbox of Rachel
INSERT INTO email_accounts (id, user_id, email_address, username, password_encrypted, smtp_host, imap_host) VALUES
 ('30000000-0000-0000-0000-000000000001', 'a0000000-0000-0000-0000-000000000002', 'rachel@example.com', 'rachel', 'x', 'smtp.test', 'imap.test'),
 ('30000000-0000-0000-0000-000000000002', 'a0000000-0000-0000-0000-000000000003', 'john@example.com', 'john', 'x', 'smtp.test', 'imap.test');
INSERT INTO email_messages (id, account_id, message_id, folder, from_address, to_addresses, subject, body_text, is_read, is_deleted, date_received) VALUES
 ('31000000-0000-0000-0000-000000000001', '30000000-0000-0000-0000-000000000001', '<m1@test>', 'INBOX', 'diana@globex.test', '{rachel@example.com}', 'Quotation for the gates', 'Hi Rachel, please send the quotation.', false, false, '{{NOW-2d}}'),
 ('31000000-0000-0000-0000-000000000002', '30000000-0000-0000-0000-000000000001', '<m2@test>', 'SENT', 'rachel@example.com', '{diana@globex.test,boss@globex.test}', 'Re: Quotation for the gates', 'Attached is the quotation.', true, false, '{{NOW-1d}}'),
 ('31000000-0000-0000-0000-000000000003', '30000000-0000-0000-0000-000000000001', '<m3@test>', NULL, 'news@vendor.test', NULL, 'Old newsletter', NULL, true, false, '{{NOW-60d}}'),
 ('31000000-0000-0000-0000-000000000004', '30000000-0000-0000-0000-000000000002', '<m4@test>', 'INBOX', 'diana@globex.test', '{john@example.com}', 'For John only', 'Not Rachel''s mail.', false, false, '{{NOW-2d}}'),
 ('31000000-0000-0000-0000-000000000005', '30000000-0000-0000-0000-000000000001', '<m5@test>', 'INBOX', 'spam@x.test', '{rachel@example.com}', 'Deleted', 'gone', false, true, '{{NOW-3d}}');

-- shared CRM inbox
INSERT INTO crm_email_threads (id, subject, linked_contact_id) VALUES
 ('40000000-0000-0000-0000-000000000001', 'Quotation for the gates', 'c0000000-0000-0000-0000-000000000001'),
 ('40000000-0000-0000-0000-000000000002', 'Re: Outbound only', NULL);
INSERT INTO crm_email_messages (id, thread_id, message_id_header, direction, from_email, from_name, to_emails, subject, body_text, is_read, received_at) VALUES
 ('41000000-0000-0000-0000-000000000001', '40000000-0000-0000-0000-000000000001', '<c1@test>', 'inbound', 'diana@globex.test', 'Diana Prince', '{hello@example.com}', 'Quotation for the gates', 'Please quote 3 gates.', true, '{{NOW-3d}}'),
 ('41000000-0000-0000-0000-000000000002', '40000000-0000-0000-0000-000000000001', '<c2@test>', 'outbound', 'hello@example.com', 'ACME', '{diana@globex.test}', 'Re: Quotation for the gates', 'Here it is.', true, '{{NOW-2d}}'),
 ('41000000-0000-0000-0000-000000000003', '40000000-0000-0000-0000-000000000001', '<c3@test>', 'inbound', 'diana@globex.test', 'Diana Prince', '{hello@example.com}', 'Re: Quotation for the gates', 'Thanks, can you add Care?', false, '{{NOW-1d}}'),
 ('41000000-0000-0000-0000-000000000004', '40000000-0000-0000-0000-000000000002', '<c4@test>', 'outbound', 'hello@example.com', NULL, '{someone@acme.test}', 'Outbound only', NULL, true, '{{NOW-5d}}');
INSERT INTO crm_email_attachments (id, message_id, filename, data) VALUES
 ('42000000-0000-0000-0000-000000000001', '41000000-0000-0000-0000-000000000003', 'po-draft.pdf', '\x00');

-- follow-up reminders
INSERT INTO crm_reminders (id, owner_user_id, source, entity_type, entity_id, chat_id, title, note, due_at, status, snooze_count, created_at) VALUES
 ('50000000-0000-0000-0000-000000000001', 'a0000000-0000-0000-0000-000000000002', 'manual', 'deal', 'e0000000-0000-0000-0000-000000000001', NULL, 'Chase the quote', 'They wanted Care', '2020-03-01 03:00:00+00', 'open', 0, '2020-01-01 00:00:00+00'),
 ('50000000-0000-0000-0000-000000000002', 'a0000000-0000-0000-0000-000000000002', 'no_reply', 'whatsapp_chat', NULL, '1555010001111@c.us', 'Reply to Diana', NULL, '2035-05-01 02:30:00+00', 'open', 0, '2020-01-01 00:00:00+00'),
 ('50000000-0000-0000-0000-000000000003', 'a0000000-0000-0000-0000-000000000002', 'manual', 'lead', 'd0000000-0000-0000-0000-000000000001', NULL, 'Done already', NULL, '2020-02-01 03:00:00+00', 'done', 0, '2020-01-01 00:00:00+00'),
 ('50000000-0000-0000-0000-000000000004', 'a0000000-0000-0000-0000-000000000003', 'manual', 'contact', 'c0000000-0000-0000-0000-000000000002', NULL, 'Not yours', NULL, '2020-03-01 03:00:00+00', 'open', 0, '2020-01-01 00:00:00+00');

-- meetings
INSERT INTO nary_meetings (id, deal_id, lead_id, contact_id, chat_id, channel, slots, status, starts_at, ends_at, event_id, meet_link, attendee_email, title, origin, attendees) VALUES
 ('60000000-0000-0000-0000-000000000001', 'e0000000-0000-0000-0000-000000000001', NULL, 'c0000000-0000-0000-0000-000000000001', '1555010001111@c.us', 'whatsapp', '[]', 'booked', '2031-03-10 04:00:00+00', '2031-03-10 04:30:00+00', 'evt-1', 'https://meet.google.com/abc-defg-hij', 'diana@globex.test', 'Acme Ltd x Globex Corp', 'request', '[]'),
 ('60000000-0000-0000-0000-000000000002', NULL, 'd0000000-0000-0000-0000-000000000002', NULL, NULL, 'email', '[]', 'cancelled', '2031-04-01 03:00:00+00', NULL, 'evt-2', NULL, NULL, NULL, 'slots', '[]'),
 ('60000000-0000-0000-0000-000000000003', NULL, NULL, NULL, NULL, 'email', '[]', 'booked', '2031-05-01 02:00:00+00', NULL, NULL, NULL, NULL, NULL, 'slots', '["a@x.test", "b@x.test"]'),
 ('60000000-0000-0000-0000-000000000004', NULL, NULL, NULL, NULL, 'whatsapp', '[]', 'proposed', NULL, NULL, NULL, NULL, NULL, 'Not booked yet', 'slots', '[]');

-- WhatsApp chats Nary knows (an inbound message each where the customer has written)
INSERT INTO wa_chats (id, chat_id, phone, display_name, is_my_contact, status, customer_started, contact_id, staff_user_id, human_until, attribution_asked) VALUES
 ('70000000-0000-0000-0000-000000000001', '1555010001111@c.us', '+1555010001111', 'Diana P', false, 'active', true, 'c0000000-0000-0000-0000-000000000001', NULL, NULL, false),
 ('70000000-0000-0000-0000-000000000002', '1555010003333@c.us', '+1555010003333', 'Rachel HP', false, 'active', false, NULL, 'a0000000-0000-0000-0000-000000000002', NULL, false),
 ('70000000-0000-0000-0000-000000000003', '1555010004444@c.us', '+1555010004444', 'Mama', false, 'excluded', true, NULL, NULL, NULL, false),
 ('70000000-0000-0000-0000-000000000004', '1555010005555@c.us', '+1555010005555', 'Mr Held', false, 'held', true, NULL, NULL, NULL, false),
 ('70000000-0000-0000-0000-000000000005', '1555010006666@c.us', '+1555010006666', '~ab', false, 'active', true, NULL, NULL, '2099-01-01 00:00:00+00', false),
 ('70000000-0000-0000-0000-000000000006', '1555010007777@c.us', NULL, NULL, false, 'active', true, NULL, NULL, NULL, false);
INSERT INTO wa_messages (id, message_key, chat_id, direction, from_me, body) VALUES
 ('71000000-0000-0000-0000-000000000001', 'GOLDKEY0001', '1555010001111@c.us', 'inbound', false, 'Hello'),
 ('71000000-0000-0000-0000-000000000002', 'GOLDKEY0002', '1555010004444@c.us', 'inbound', false, 'Hey'),
 ('71000000-0000-0000-0000-000000000003', 'GOLDKEY0003', '1555010005555@c.us', 'inbound', false, 'Mr'),
 ('71000000-0000-0000-0000-000000000004', 'GOLDKEY0004', '1555010006666@c.us', 'inbound', false, 'Hi'),
 ('71000000-0000-0000-0000-000000000005', 'GOLDKEY0005', '1555010007777@c.us', 'inbound', false, 'Yo');
