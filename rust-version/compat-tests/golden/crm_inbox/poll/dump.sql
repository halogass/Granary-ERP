-- The rows a poll leaves, normalised (ids by order, recent timestamps masked), as one JSON document.
-- "recent" = written during this run: now() at poll time, or a {{RECENT}} Date header (now - 1 hour). The window
-- is 6 hours so it never reaches the fixed dates in mail/*.eml (all before 14 Oct 2026): the dump does not
-- depend on the day it runs (a '2 days' window masked the 13 Oct mail on the recording day only).
SET TIME ZONE 'UTC';
WITH m AS (
  SELECT *, row_number() OVER (ORDER BY imap_folder NULLS FIRST, imap_uid NULLS FIRST, message_id_header) AS n
  FROM crm_email_messages
), t AS (
  SELECT thread_id, row_number() OVER (ORDER BY min(n)) AS tn FROM m GROUP BY thread_id
), ts AS (
  SELECT 1
)
SELECT json_build_object(
  'messages', (SELECT coalesce(json_agg(json_build_object(
      'thread', t.tn, 'message_id_header', m.message_id_header, 'in_reply_to', m.in_reply_to,
      'references', m."references", 'direction', m.direction, 'from_email', m.from_email, 'from_name', m.from_name,
      'to_emails', m.to_emails, 'cc_emails', m.cc_emails, 'subject', m.subject, 'body_html', m.body_html,
      'body_text', m.body_text, 'imap_uid', m.imap_uid, 'imap_folder', m.imap_folder, 'reply_to', m.reply_to,
      'auth', m.auth, 'is_read', m.is_read,
      'received_at', CASE WHEN m.received_at > now() - interval '6 hours' THEN 'recent' ELSE m.received_at::text END,
      'sent_by_id', m.sent_by_id, 'sent_by_nary', m.sent_by_nary,
      'attachments', (SELECT coalesce(json_agg(json_build_object('filename', a.filename, 'content_type', a.content_type,
          'size_bytes', a.size_bytes, 'md5', md5(a.data)) ORDER BY a.filename, md5(a.data)), '[]'::json)
          FROM crm_email_attachments a WHERE a.message_id = m.id)
    ) ORDER BY m.n), '[]'::json) FROM m JOIN t ON t.thread_id = m.thread_id),
  'threads', (SELECT coalesce(json_agg(json_build_object(
      'thread', t.tn, 'subject', th.subject, 'status', th.status, 'is_read', th.is_read,
      'message_count', th.message_count, 'last_sender_email', th.last_sender_email,
      'last_sender_name', th.last_sender_name,
      'last_message_at', CASE WHEN th.last_message_at > now() - interval '6 hours' THEN 'recent' ELSE th.last_message_at::text END,
      'linked_contact_id', th.linked_contact_id, 'linked_org_id', th.linked_org_id,
      'linked_lead_id', th.linked_lead_id, 'linked_deal_id', th.linked_deal_id
    ) ORDER BY t.tn), '[]'::json) FROM crm_email_threads th JOIN t ON t.thread_id = th.id),
  'orphan_threads', (SELECT count(*) FROM crm_email_threads th WHERE NOT EXISTS (SELECT 1 FROM crm_email_messages x WHERE x.thread_id = th.id)),
  'activities', (SELECT coalesce(json_agg(json_build_object(
      'entity_type', a.entity_type, 'entity_id', a.entity_id, 'contact_id', a.contact_id, 'lead_id', a.lead_id,
      'deal_id', a.deal_id, 'activity_type', a.activity_type, 'direction', a.direction, 'subject', a.subject,
      'content', a.content, 'is_completed', a.is_completed, 'channel_message_id', a.channel_message_id,
      'completed_at', CASE WHEN a.completed_at > now() - interval '6 hours' THEN 'recent' ELSE a.completed_at::text END,
      'created_by_id', a.created_by_id
    ) ORDER BY a.channel_message_id, a.entity_type), '[]'::json) FROM activities a WHERE a.activity_type = 'email'),
  'events', (SELECT coalesce(json_agg(json_build_object(
      'type', e.type, 'entity_type', e.entity_type,
      'thread', (SELECT t.tn FROM t WHERE t.thread_id = e.entity_id),
      'message', (SELECT m.message_id_header FROM m WHERE m.id::text = e.payload->>'message_id'),
      'lead_id', e.payload->'lead_id', 'deal_id', e.payload->'deal_id', 'contact_id', e.payload->'contact_id',
      'from', e.payload->'from', 'keys', (SELECT json_agg(k ORDER BY k) FROM json_object_keys(e.payload::json) k)
    ) ORDER BY e.type, e.payload->>'from'), '[]'::json) FROM domain_events e WHERE e.type LIKE 'email.%'),
  'junk_mark', (SELECT value FROM system_settings WHERE key = 'nary.junk_last_uid'),
  'skipped_marks', (SELECT coalesce(json_agg(json_build_object('key', key, 'value', value, 'value_type', value_type,
      'category', category, 'label', label) ORDER BY key), '[]'::json)
      FROM system_settings WHERE key LIKE 'crm_inbox.skipped_uid.%')
)::text;
