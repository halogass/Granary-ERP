/**
 * The settings registry: area -> section -> settings and custom blocks. The Settings page (S2)
 * renders from it and the command palette searches it (`settingsSearchEntries`, keep its shape).
 * Every setting the ERP edits lives in exactly one section; a value the server has that is not
 * listed here (and not managed by a block or another page) shows under System & security, Other.
 */

export type SettingType =
  | "text" | "number" | "money" | "percent" | "boolean" | "select" | "textarea" | "json"
  | "secret" | "email" | "url" | "time" | "date";

export interface SettingDef {
  key: string;
  label: string;
  hint?: string;
  type: SettingType;
  /** The value the system ships with, as stored (a string), when known. */
  default?: string;
  unit?: string;
  options?: { value: string; label: string }[];
  min?: number;
  max?: number;
  step?: number;
  /** Cannot be saved empty (payroll refuses to run without it). */
  required?: boolean;
  /** Column headings for a JSON table editor (brackets: [upper bound, rate]). */
  columns?: string[];
}

/** Custom blocks the Settings page renders (forms with their own API, lists, uploads). */
export const SETTINGS_BLOCKS = [
  "company.logo", "company.favicon", "company.login-text", "company.letterhead", "company.document-categories",
  "sales.crm-attributes", "sales.followups", "sales.rate-card", "sales.deleted-activities",
  "money.currency", "money.taxes",
  "people.offices", "people.kiosks",
  "ai.general", "ai.keys", "ai.routing", "ai.budget", "ai.persona", "ai.research", "ai.enrichment",
  "ai.scoring", "ai.knowledge", "nary.ack-templates",
  "messaging.signature-template", "messaging.test-address", "messaging.system-smtp", "messaging.crm-inboxes",
  "approvals.policies",
  "integrations.mcp", "integrations.users", "integrations.gitlab",
  "system.other",
] as const;
export type SettingsBlockId = (typeof SETTINGS_BLOCKS)[number];

export interface SettingsSection {
  id: string;
  label: string;
  description?: string;
  settings: SettingDef[];
  /** A custom block rendered under the section's settings. */
  block?: SettingsBlockId;
  /** What the block edits, so search finds it (label, hint, key). */
  blockFields?: { label: string; hint?: string; key?: string }[];
  /** Words that should find this section (old tab names...). */
  keywords?: string[];
}

export interface SettingsArea {
  id: string;
  label: string;
  description: string;
  sections: SettingsSection[];
}

type SeedType = "n" | "b" | "j" | "t" | "s";

/** Type and shipped default of every seeded setting (rust-version seed data and migrations). */
const SEED: Record<string, [SeedType, string?]> = {
  "company_name": ["s", "Your Company Ltd"],
  "company_trade_name": ["s", "Your Company"],
  "company_email": ["s", "hello@example.com"],
  "company_phone": ["s"],
  "company_website": ["s", "https://example.com"],
  "company_address": ["s"],
  "company_rd_office": ["s"],
  "company_tax_id": ["s"],
  "company_registration_no": ["s"],
  "company_signatory": ["s"],
  "company_signatory_title": ["s", ""],
  "invoice_due_days": ["n", "14"],
  "quotation_validity_days": ["n", "30"],
  "max_discount_pct_without_approval": ["n", "15"],
  "email_smtp_host": ["s", "smtp.gmail.com"],
  "email_smtp_port": ["n", "587"],
  "email_smtp_user": ["s", ""],
  "email_smtp_password": ["s"],
  "email_smtp_from_name": ["s", "Granary"],
  "email_smtp_from_email": ["s", ""],
  "email_smtp_tls": ["b", "true"],
  "crm_smtp_host": ["s", "smtp.gmail.com"],
  "crm_smtp_port": ["n", "587"],
  "crm_smtp_user": ["s", ""],
  "crm_smtp_password": ["s"],
  "crm_smtp_from_name": ["s", "Your Company"],
  "crm_smtp_from_email": ["s", ""],
  "crm_smtp_tls": ["b", "true"],
  "crm_imap_host": ["s", "imap.gmail.com"],
  "crm_imap_port": ["n", "993"],
  "crm_imap_user": ["s", ""],
  "crm_imap_password": ["s"],
  "crm_imap_ssl": ["b", "true"],
  "wa_session_auto_reconnect": ["b", "true"],
  "branding.logo_type": ["s", "text"],
  "branding.logo_text": ["s", "Granary"],
  "branding.logo_subtitle": ["s", "Granary"],
  "branding.logo_image_data": ["s"],
  "branding.favicon_data": ["s"],
  "timezone": ["s", "UTC"],
  "default_currency": ["s", "USD"],
  "date_format": ["s", "DD/MM/YYYY"],
  "quotation_prefix": ["s", "QUO"],
  "invoice_prefix": ["s", "INV"],
  "proforma_prefix": ["s", "PRO"],
  "payment_terms_default": ["n", "14"],
  "hr.face_match_distance": ["n", "0.50"],
  "hr.geo_max_accuracy_m": ["n", "100"],
  "hr.work_start_time": ["t", "09:00"],
  "payroll.contributions": ["j", "[]"],
  "payroll.income_tax": ["j", '{"method": "none"}'],
  "finance.gl_autopost_from": ["s", ""],
  "procurement.default_withholding_rate": ["n", "0"],
  "procurement.receipt_over_tolerance_pct": ["n", "5"],
  "sign.signature_max_bytes": ["n", "500000"],
  "payroll.overtime_hours_divisor": ["n", "173"],
  "payroll.overtime_multipliers": ["j"],
  "payroll.rest_weekdays": ["j"],
  "marketing.email_daily_limit": ["n", "100"],
  "marketing.email_min_delay_seconds": ["n", "30"],
  "marketing.email_max_delay_seconds": ["n", "60"],
  "marketing.queue_lock_ttl_seconds": ["n", "180"],
  "mail.sync_lock_ttl_seconds": ["n", "600"],
  "auth.login_max_attempts": ["n", "10"],
  "auth.login_window_seconds": ["n", "60"],
  // 2 Nov 2026: two-factor sign-in (TOTP, migration 0148)
  "auth.totp.issuer": ["s", "Granary"],
  "auth.totp.challenge_seconds": ["n", "300"],
  "auth.totp.max_failed_attempts": ["n", "5"],
  "auth.totp.failed_window_seconds": ["n", "900"],
  "auth.totp.recovery_codes": ["n", "10"],
  "sign.reminder_window_hours": ["n", "48"],
  "sign.reminder_interval_hours": ["n", "24"],
  // 3 Nov 2026: every approved outbound PDF carries the company's digital seal (migration 0152)
  "documents.seal.outbound": ["b", "true"],
  "catalog.conversion_base_currency": ["s", "USD"],
  "catalog.independent_currencies": ["j"],
  "sales.split_payment_min_unit_equivalents": ["n", "21"],
  "sales.split_payment_on_order_pct": ["n", "60"],
  "sales.service_down_payment_pct": ["n", "50"],
  "fleet.expiry_warning_days": ["n", "30"],
  "maintenance.due_soon_days": ["n", "7"],
  "hr.overnight_shift_hours": ["n", "16"],
  "hr.face.step_timeouts_ms": ["j", '{"find": 8000, "action": 10000, "return": 5000}'],
  "hr.face.challenge_ttl_seconds": ["n", "60"],
  // 6 Nov 2026: passive face check first (screen colour flash + anti-spoof), active as fallback (migration 0166)
  "hr.face.passive.enabled": ["b", "true"],
  "hr.face.passive.flash_count": ["n", "5"],
  // 6 Nov 2026 (migration 0167): later frame grab for lagging phone cameras, chromaticity units for the delta
  "hr.face.passive.flash_ms": ["n", "400"],
  "hr.face.passive.settle_ms": ["n", "340"],
  "hr.face.passive.palette": ["j", "[[255,120,120],[120,255,140],[120,150,255],[255,230,120],[220,130,255],[120,240,240]]"],
  "hr.face.passive.flash_min_delta": ["n", "1.5"],
  "hr.face.passive.flash_min_corr": ["n", "0.6"],
  "hr.face.passive.spoof_min_real": ["n", "0.7"],
  "crm.followup.due_hour": ["n", "9"],
  "crm.followup.escalate_after_hours": ["n", "24"],
  "projects.monthly_hours": ["n", "173"],
  "projects.default_hourly_cost": ["n", ""],
  "finance.invoice_reminder_days": ["j"],
  "finance.invoice_reminder_repeat_days": ["n", "30"],
  "finance.default_sales_tax": ["j"],
  "currency.decimals": ["j"],
  "crm.scheduled.max_attempts": ["n", "12"],
  "crm.scheduled.stuck_after_minutes": ["n", "30"],
  "crm.followup.default_delay_days": ["n", "3"],
  "crm.followup.no_reply_days": ["n", "3"],
  "crm.followup.lead_idle_days": ["n", "7"],
  "crm.followup.deal_stage_idle_days": ["n", "14"],
  "crm.followup.quotation_followup_days": ["n", "5"],
  "crm.followup.max_per_rule_pass": ["n", "25"],
  "crm.followup.quiet_from": ["t", "08:00"],
  "crm.followup.quiet_to": ["t", "20:00"],
  "crm.followup.digest_weekday": ["n", "1"],
  "crm.followup.digest_time": ["t", "08:00"],
  "crm.followup.rule_no_reply": ["b", "true"],
  "crm.followup.rule_stale_lead": ["b", "true"],
  "crm.followup.rule_stale_deal": ["b", "true"],
  "crm.followup.rule_quotation": ["b", "true"],
  "crm.funnel.stage_probabilities": ["j"],
  "crm.funnel.loss_reasons": ["j"],
  "crm.funnel.disqualify_reasons": ["j"],
  "crm.funnel.lead_sources": ["j"],
  "crm.funnel.default_lead_owner_email": ["s", ""],
  "crm.followup.rule_next_action": ["b", "true"],
  "ai.routing": ["j"],
  "ai.pricing": ["j"],
  "ai.pricing.batch_discount_pct": ["n", "50"],
  "ai.budget.warn_pct": ["n", "80"],
  "ai.budget.stop_pct": ["n", "100"],
  "ai.budget.hard_stop_pct": ["n", "150"],
  "ai.budget.essential_jobs": ["j"],
  "ai.budget.job_caps": ["j"],
  "ai.budget.monthly_usd": ["n", "100"],
  "nary.paused": ["b", "false"],
  "nary.shadow_mode": ["b", "true"],
  "nary.autonomy.levels": ["j"],
  "nary.autonomy.promotion_count": ["n", "10"],
  "nary.autonomy.edit_threshold_pct": ["n", "10"],
  "nary.autonomy.demotion_floor": ["n", "1"],
  "nary.autonomy.never_promote": ["j"],
  "nary.autonomy.never_promote_unlocked": ["b", "false"],
  "nary.working_hours": ["j"],
  "nary.heartbeat_minutes": ["n", "30"],
  "nary.brief_times": ["j"],
  "nary.urgent": ["j"],
  "nary.events.batch_size": ["n", "50"],
  "nary.events.max_attempts": ["n", "5"],
  "nary.events.resume_max_age_hours": ["n", "24"],
  "nary.pending.expire_days": ["n", "7"],
  "nary.apply_wait_seconds": ["n", "45"],
  "nary.applying_stale_minutes": ["n", "30"],
  "nary.errors.dedupe_hours": ["n", "24"],
  "nary.shadow_exempt_actions": ["j"],
  "nary.ack.enabled": ["b", "false"],
  "nary.ack.templates": ["j"],
  "nary.ack.default_language": ["s", "id"],
  "nary.ack.ongoing_days": ["n", "30"],
  "nary.ack.late_after_minutes": ["n", "30"],
  "nary.intake.arrival_window_minutes": ["n", "120"],
  "nary.intake.currency_by_country": ["j"],
  "nary.web3forms.sender_domains": ["j"],
  "nary.web3forms.dkim_domains": ["j"],
  "nary.web3forms.known_fields": ["j"],
  "nary.web3forms.required_fields": ["j"],
  "nary.web3forms.routes": ["j"],
  "nary.web3forms.interest_map": ["j"],
  "nary.web3forms.poll_junk": ["b", "true"],
  "nary.identity.default_country": ["s", "US"],
  "nary.identity.prefill_labels": ["j"],
  "nary.identity.thresholds": ["j"],
  "nary.identity.signal_scores": ["j"],
  "nary.identity.honorifics": ["j"],
  "nary.identity.recent_form_days": ["n", "14"],
  "nary.triage.example_count": ["n", "20"],
  "nary.triage.business_signals": ["j"],
  "nary.wa.signature": ["s", "_\u2014 Nary_"],
  "nary.wa.max_per_hour": ["n", "20"],
  "nary.wa.max_per_day": ["n", "100"],
  "nary.wa.takeover_hours": ["n", "24"],
  "nary.wa.gapcheck_minutes": ["n", "30"],
  "nary.wa.gapcheck_lookback_hours": ["n", "48"],
  "nary.wa.health_minutes": ["n", "10"],
  "nary.wa.health_backlog_minutes": ["n", "15"],
  "nary.wa.media_max_mb": ["n", "25"],
  "nary.wa.card_notices": ["b", "true"],
  "nary.wa.labels_enabled": ["b", "false"],
  "nary.wa.label_map": ["j"],
  "nary.history.full_read_months": ["n", "6"],
  "nary.history.chats_per_run": ["n", "3"],
  "nary.history.page_size": ["n", "100"],
  "nary.history.pause_seconds": ["n", "5"],
  "nary.history.run_minutes": ["n", "10"],
  "nary.history.pages_per_run": ["n", "10"],
  "nary.history.batch_size": ["n", "100"],
  "nary.history.sync_attempts": ["n", "2"],
  "nary.history.style_sample": ["n", "200"],
  "nary.history.missed_after_days": ["n", "14"],
  "nary.stt.max_seconds": ["n", "600"],
  "nary.score.rubric": ["j"],
  "nary.score.classes": ["j"],
  "nary.score.boundary_margin": ["n", "1"],
  "nary.score.red_flags": ["j"],
  "nary.score.green_flags": ["j"],
  "nary.reply.questions": ["j"],
  "nary.reply.max_questions": ["n", "2"],
  "nary.reply.max_lines": ["n", "5"],
  "nary.price.catalog_terms": ["j"],
  "nary.email.reply_window_hours": ["n", "48"],
  "nary.cadence.steps": ["j"],
  "nary.cadence.min_gap_days": ["n", "2"],
  "nary.cadence.max_without_news": ["n", "2"],
  "nary.cadence.at_risk_silent_days": ["n", "7"],
  "nary.cadence.nurture_silent_days": ["n", "14"],
  "nary.cadence.recontact_days": ["n", "60"],
  "nary.negotiation.scripts": ["j"],
  "nary.negotiation.competitor_checklist": ["j"],
  "sales.margins": ["j"],
  "nary.contract.nudge_days": ["n", "3"],
  "nary.contract.won_needs_dp": ["b", "true"],
  "nary.google.client_id": ["s", ""],
  "nary.google.client_secret": ["s"],
  "nary.google.account_email": ["s", ""],
  "nary.google.calendar_id": ["s", "primary"],
  "nary.meetings.rules": ["j"],
  "nary.meetings.holidays": ["j"],
  "nary.meetings.reduced_hours_periods": ["j"],
  "nary.meetings.customer_hours": ["j"],
  "nary.meetings.text": ["j"],
  "nary.meetings.reminders": ["j"],
  "nary.handoff.kickoff_tasks": ["j"],
  "nary.partners.routing": ["j"],
  "nary.partners.warn_days": ["n", "14"],
  "nary.report.kpi_targets": ["j"],
  "nary.report.weekday": ["n", "1"],
  "nary.report.window_days": ["n", "90"],
  "nary.report.response_sla_minutes": ["n", "120"],
  "nary.sop_revisions.briefs": ["j"],
  "nary.sop_revisions.codes": ["j"],
  "documents.proposal.template": ["j"],
  "documents.proposal.labels": ["j"],
  "documents.proposal.validity_days": ["n", "30"],
  "documents.proposal.classification": ["j"],
  "documents.proposal.prepared_by": ["j"],
  "proposal.discovery.fields": ["j"],
  "proposal.discovery.min_coverage_pct": ["n", "60"],
  "proposal.cost.contingency_pct": ["n", "15"],
  "proposal.cost.days_per_month": ["n", "21"],
  "proposal.cost.load_factor": ["n", "1.2"],
  "proposal.cost.min_monthly": ["n", "5300000"],
  "proposal.cost.update_threshold_pct": ["n", "10"],
  "nary.staff_chat.enabled": ["b", "true"],
  "nary.staff_chat.session_hours": ["n", "12"],
  "nary.staff_chat.max_bubbles": ["n", "3"],
  "nary.staff_chat.bubble_chars": ["n", "1500"],
  "nary.staff_chat.voice": ["b", "true"],
  "nary.owner.enabled": ["b", "true"],
  "nary.owner.urgent_kinds": ["j"],
  "nary.owner.bundle_minutes": ["n", "5"],
  "nary.owner.remind_working_hours": ["n", "3"],
  "nary.owner.default_language": ["s", "id"],
  "nary.owner.consult_quotations": ["b", "true"],
  "nary.owner.consult_proposals": ["b", "true"],
  "nary.owner.topics_in_context": ["n", "10"],
  "nary.email.intake_enabled": ["b", "true"],
  "nary.email.skip_rules": ["j"],
  "nary.email.signature_fields": ["j"],
  "crm.funnel.lead_priorities": ["j"],
  "crm.segments": ["j", "[]"],
  "crm.activity_types": ["j"],
  // 23 Oct: the longest note the CRM stores (migration 0125)
  "crm.activity.max_chars": ["n", "200000"],
  "nary.staff_chat.max_lines": ["n", "8"],
  "nary.owner.one_at_a_time": ["b", "true"],
  "nary.briefs.items_per_section": ["n", "3"],
  "ai.assistant.catalog_digest_max_items": ["n", "300"],
  "ai.erp_guide.enabled": ["b", "true"],
  "nary.skills.max_in_prompt": ["n", "30"],
  "nary.skills.tool_ideas_project": ["s", ""],
  "nary.style.refresh_days": ["n", "30"],
  "mcp.oauth.access_minutes": ["n", "60"],
  "mcp.oauth.refresh_days": ["n", "30"],
  "mcp.oauth.code_minutes": ["n", "10"],
  "mcp.pat.default_days": ["n", "90"],
  "mcp.pat.max_days_power": ["n", "90"],
  "mcp.owner.confirm_minutes": ["n", "10"],
  // stage 8: nary_apply needs the person's TOTP code for sends, money, deletes and merges
  "mcp.apply.require_totp": ["b", "true"],
  // 3 Nov 2026: the work queue and the Owner's nudge (migration 0154)
  "nary.queue.nudge_enabled": ["b", "true"],
  "nary.queue.nudge_times": ["j", "[\"08:00\", \"13:00\", \"17:00\"]"],
  "nary.queue.urgent_kinds": ["j", "[\"whatsapp_reply\", \"email_reply\", \"new_lead\", \"unknown_chat\"]"],
  "nary.queue.today_kinds": ["j", "[\"followup_due\", \"card_expiring\", \"meeting_reminder\", \"meeting_brief\", \"accepted_no_contract\", \"ready_to_win\", \"approval_overdue\", \"quotation_expiring\"]"],
  "nary.queue.urgent_gap_minutes": ["n", "30"],
  "nary.queue.nudge_items": ["n", "5"],
  "nary.queue.default_limit": ["n", "50"],
  "nary.queue.quotation_expiry_days": ["n", "7"],
  "nary.queue.meeting_reminder_hours": ["n", "36"],
  "nary.queue.meeting_brief_hours": ["n", "3"],
  "nary.queue.card_expiry_hours": ["n", "24"],
  "nary.queue.new_lead_days": ["n", "14"],
  "manufacturing.label.width_mm": ["n", "62"],
  "manufacturing.label.height_mm": ["n", "29"],
  "manufacturing.serial.generate_tries": ["n", "5"],
  "nary.staff_chat.wait_seconds": ["n", "120"],
  "nary.staff_chat.late_reply_hours": ["n", "24"],
  "nary.wa.gapcheck_grace_seconds": ["n", "120"],
  "nary.owner.quiet_minutes": ["n", "3"],
  "nary.owner.alert_stale_hours": ["n", "6"],
  "nary.owner.confirm_twice_actions": ["j"],
  "crm.followup.digest_max_items": ["n", "10"],
  "sales.quotation.expire_grace_days": ["n", "0"],
  // 2 Nov 2026: quotation tax notes (migration 0147)
  "sales.quotation.tax_exclusive_note": ["s", "Prices exclude sales tax and withholding tax."],
  "sales.quotation.tax_exclusive_phrases": ["j", '["exclude vat", "excludes vat", "excluding vat", "exclusive of vat", "excl. vat", "excl vat", "exclude tax", "excludes tax", "excluding tax", "exclusive of tax", "before tax", "exclude sales tax", "excluding sales tax", "plus vat", "plus tax"]'],
  "sales.quotation.tax_inclusive_phrases": ["j", '["include vat", "includes vat", "including vat", "inclusive of vat", "incl. vat", "incl vat", "include tax", "includes tax", "including tax", "inclusive of tax", "tax included", "vat included", "including sales tax"]'],
  "sales.quotation.expired_nudge_days": ["n", "14"],
  // 4 Nov 2026: the house quotation notes templates (migration 0158)
  "sales.quotation.notes_template.products": ["j"],
  "sales.quotation.notes_template.services": ["j"],
  "sales.quotation.notes_template.subscription": ["j"],
  "sales.quotation.notes_families": ["j", '{"order": ["subscription", "services", "products"], "fallback": "products"}'],
  // 5 Nov 2026: more quotation notes templates (migration 0161)
  "sales.quotation.notes.texts": ["j"],
  "sales.quotation.notes.check_phrases": ["j"],
  "sales.quotation.notes.lead_days_min": ["n", "10"],
  "sales.quotation.notes.lead_days_max": ["n", "15"],
  "sales.quotation.notes.invoice_due_days": ["n", "14"],
  "sales.quotation.notes.non_payment_pause_days": ["n", "30"],
  "sales.quotation.notes.non_payment_terminate_days": ["n", "60"],
  "sales.quotation.notes.warranty_years": ["n", "1"],
  "sales.quotation.notes.block_submit": ["b", "true"],
  // 5 Nov 2026: the pricing rules (E7) and the Safety Pack scope (migration 0159)
  "sales.pricing_rules.min_cost_multiple": ["n", "2.0"],
  "sales.pricing_rules.block_submit": ["b", "true"],
  "sales.pricing_rules.owner_approval_skus": ["j", "[]"],
  "nary.reply_alerts.enabled": ["b", "true"],
  "nary.reply_alerts.mode": ["s", "action_needed"],
  "nary.staff_chat.card_window_minutes": ["n", "60"],
  "crm.enrich.on_create": ["b", "true"],
  "crm.enrich.fields": ["s"],
  "crm.enrich.min_confidence": ["n", "70"],
  "crm.enrich.refresh_days": ["n", "30"],
  "crm.enrich.daily_limit": ["n", "50"],
  "currency.functional": ["s", "USD"],
  "currency.active": ["j"],
  "currency.api_key": ["s"],
  "ai.anthropic.api_key": ["s"],
  "nary.reply.backfill_window_hours": ["n", "24"],
  "nary.reply.debounce_seconds": ["n", "75"],
  "nary.wa.chat_max_messages": ["n", "2"],
  "nary.wa.chat_window_minutes": ["n", "10"],
  "nary.wa.burst_alert_messages": ["n", "3"],
  "nary.wa.burst_alert_minutes": ["n", "5"],
  "nary.wa.complaint_pause_hours": ["n", "72"],
  "nary.reply.unanswered_business_hours": ["n", "2"],
  "nary.reply.unanswered_lookback_days": ["n", "14"],
  "nary.wa.greeting_hours": ["j"],
  "nary.writing.lint_enabled": ["b", "true"],
  "nary.briefs.snooze_days": ["n", "3"],
  "nary.send_at.max_days": ["n", "30"],
  "nary.meetings.recording_max_minutes": ["n", "240"],
  "nary.meetings.recording_max_mb": ["n", "500"],
  "nary.meetings.notes_max_mb": ["n", "40"],
  "nary.meetings.recording_poll_seconds": ["n", "45"],
  "nary.meetings.recording_deadline_hours": ["n", "12"],
  "nary.meetings.recording_resubmits": ["n", "2"],
  // 3 Nov 2026: find_meeting_slots (migration 0156)
  "nary.meetings.find_max_days": ["n", "30"],
  "nary.meetings.find_max_slots": ["n", "8"],
  "nary.mailboxes.notify_owner": ["b", "false"],
  // batch 4 D2
  "nary.attach.max_files": ["n", "5"],
  "nary.attach.max_total_mb": ["n", "20"],
  // 29 Oct 2026: upload_file and preview_visual save_to
  "nary.upload.max_mb": ["n", "10"],
  // 1 Nov 2026: videos (migration 0145)
  "nary.upload.video_max_mb": ["n", "100"],
  "nary.wa.video_max_mb": ["n", "16"],
  "nary.email.link_days": ["n", "7"],
  "nary.wa.media_timeout_seconds": ["n", "240"],
  // 5 Nov 2026: one-time upload links (migration 0163)
  "files.upload_link.minutes": ["n", "30"],
  "files.upload_link.max_files": ["n", "10"],
  "files.upload_link.requests_per_minute": ["n", "60"],
  // batch 4 D1
  "nary.files.max_mb": ["n", "25"],
  "nary.files.vision_min_chars": ["n", "30"],
  "nary.files.vision_max_pages": ["n", "30"],
  "nary.files.page_cap": ["n", "200"],
  "nary.files.sheet_max_rows": ["n", "200"],
  "nary.files.read_max_chars": ["n", "12000"],
  // multi-inbox: automatic-message pacing, duplicate deals, meeting offers
  "nary.autosend.min_gap_hours": ["n", "12"],
  "crm.deal_dup.window_days": ["n", "30"],
  "nary.writing.meeting_offer_patterns": ["j"],
  // security 27 Oct (migration 0138): email relink sweep, duplicate pairs asked again
  "crm.email.relink_days": ["n", "180"],
  "crm.email.relink_interval_minutes": ["n", "15"],
  "crm.email.relink_full_hours": ["n", "24"],
  "crm.dup.repropose_days": ["n", "30"],
  // doclib A4: the document library (migration 0117)
  "documents.max_file_mb": ["n", "100"],
  "documents.watermark.customer_text": ["s", "Prepared for {company} · {date} · Confidential"],
  "documents.watermark.staff_text": ["s", "Downloaded by {name} · {date} · Confidential · {id}"],
  "documents.watermark.font_size": ["n", "7"],
  "documents.review_reminder_days": ["n", "14"],
  "documents.price_list.document_title": ["s", "Price List"],
  // fix-walist (migration 0118)
  "whatsapp.conversations_cache_minutes": ["n", "30"],
  // visuals V4: proposal charts and diagrams (migration 0119)
  "proposal.visuals.max_attempts": ["n", "3"],
  "proposal.visuals.vision_check": ["b", "true"],
  "proposal.visuals.default_figures": ["j", '["timeline","architecture"]'],
  "proposal.visuals.custom_svg_max_kb": ["n", "300"],
  // 1 Nov 2026: direct proposal authoring and required figures (migration 0146)
  "proposal.price_text_sections": ["j", '["commercial","terms"]'],
  "proposal.required_figures_by_requirement": ["j", '{"network":"network","multi_site":"network","process_flow":"process","integration_flow":"process"}'],
  // 17 Nov 2026: R&D backlog <-> GitLab issues (migration 0179)
  "rnd.gitlab.sync_batch": ["n", "50"],
  "rnd.gitlab.pull_minutes": ["n", "15"],
  "rnd.gitlab.retry_max_minutes": ["n", "60"],
  // stage 8 security fixes: quiz attempts, public forms, the call waiting room, vendor uploads
  "partner.training.max_attempts": ["n", "5"],
  "public.forms.requests_per_minute": ["n", "10"],
  "calls.guest_wait.max_waiting": ["n", "50"],
  // batch E: call size and STUN servers (no outside STUN unless listed)
  "calls.max_participants": ["n", "10"],
  "calls.stun_urls": ["j", "[]"],
  "vendor.portal.max_upload_mb": ["n", "10"],
};

const TYPE_OF: Record<SeedType, SettingType> = { n: "number", b: "boolean", j: "json", t: "time", s: "text" };

/** One setting; type and default come from the seed unless given. */
function s(key: string, label: string, hint?: string, more: Partial<SettingDef> = {}): SettingDef {
  const seed = SEED[key];
  return {
    key, label, hint,
    type: more.type ?? (seed ? TYPE_OF[seed[0]] : "text"),
    default: more.default ?? seed?.[1],
    ...more,
  };
}

const pct = { unit: "%" };
const days = { unit: "days" };
const hours = { unit: "hours" };
const minutes = { unit: "min" };
const seconds = { unit: "s" };

export const SETTINGS_AREAS: SettingsArea[] = [
  {
    id: "company", label: "Company & branding", description: "Name, logo, documents and the look of what you send.",
    sections: [
      {
        id: "identity", label: "Company identity", description: "As it appears on quotations, invoices and letters.",
        keywords: ["general"],
        settings: [
          s("company_name", "Legal company name"),
          s("company_trade_name", "Trade name or brand"),
          s("company_email", "Company email", undefined, { type: "email" }),
          s("company_phone", "Phone or WhatsApp"),
          s("company_website", "Website", undefined, { type: "url" }),
          s("company_address", "Head office address", undefined, { type: "textarea" }),
          s("company_rd_office", "R&D office address", undefined, { type: "textarea" }),
          s("company_tax_id", "Tax ID", "The company's tax registration number."),
          s("company_registration_no", "Registration no.", "Business registration number."),
          s("company_signatory", "Authorized signatory", "Name printed under the signature."),
          s("company_signatory_title", "Signatory title", "Position printed under the name."),
        ],
      },
      {
        id: "documents", label: "Document numbers", description: "Prefixes and default terms of new documents.",
        settings: [
          s("quotation_prefix", "Quotation prefix"),
          s("invoice_prefix", "Invoice prefix"),
          s("proforma_prefix", "Proforma prefix"),
          s("payment_terms_default", "Default payment terms", "Days until a new invoice is due.", days),
        ],
      },
      {
        id: "regional", label: "Regional", description: "Time zone, currency and date format.",
        settings: [
          s("timezone", "Time zone", "Company time for schedules, briefs and reminders."),
          s("default_currency", "Default currency", "Preselected on new documents."),
          s("date_format", "Date format"),
        ],
      },
      {
        id: "logo", label: "App logo", description: "Shown on the login page, the sidebar and portal headers.",
        settings: [], block: "company.logo", keywords: ["branding"],
        blockFields: [
          { label: "Logo type", hint: "Text logo or uploaded image.", key: "branding.logo_type" },
          { label: "Logo text", key: "branding.logo_text" },
          { label: "Subtitle or app name", key: "branding.logo_subtitle" },
          { label: "Logo image", key: "branding.logo_image_data" },
        ],
      },
      {
        id: "favicon", label: "Favicon", description: "The small icon in browser tabs and bookmarks.",
        settings: [], block: "company.favicon", keywords: ["branding", "icon"],
        blockFields: [{ label: "Favicon image", key: "branding.favicon_data" }],
      },
      {
        id: "login-page", label: "Login page text", description: "Text on the public sign-in page.",
        settings: [], block: "company.login-text", keywords: ["branding"],
        blockFields: [
          { label: "Badge text" }, { label: "Form subtitle" }, { label: "Headline" }, { label: "Tagline" },
          { label: "Feature bullets" }, { label: "Left panel footer" }, { label: "Copyright line" },
        ],
      },
      {
        id: "letterhead", label: "Letterhead", description: "The header image on every printed document.",
        settings: [], block: "company.letterhead", keywords: ["branding", "print"],
        blockFields: [{ label: "Letterhead image", hint: "Top of quotations, invoices and printed articles." }],
      },
      {
        id: "knowledge-documents", label: "Knowledge & documents",
        description: "The document library: file size, watermarks, review reminders and who sees each category.",
        keywords: ["documents", "library", "watermark", "fingerprint", "brochure", "datasheet", "price list"],
        settings: [
          s("documents.max_file_mb", "Largest file", "Per uploaded PDF, image or source file.", { unit: "MB", min: 1 }),
          s("documents.watermark.customer_text", "Footer on copies sent out",
            "Customers and partners. {company}, {name}, {date} and {id} are filled in for each copy.", { type: "textarea" }),
          s("documents.watermark.staff_text", "Footer on staff copies",
            "What staff view or download. {name}, {company}, {date} and {id} are filled in.", { type: "textarea" }),
          s("documents.watermark.font_size", "Footer font size", "Small and grey at the bottom of every page.", { unit: "pt", min: 4, max: 14 }),
          s("documents.review_reminder_days", "Remind owners before review", "Days before a document's review date; one reminder a week at most.", days),
          s("documents.price_list.document_title", "Price list document title", "The internal document Generate price list adds versions to."),
        ],
        block: "company.document-categories",
        blockFields: [{ label: "Category visibility", hint: "Which roles see each document category." }],
      },
    ],
  },
  {
    id: "sales", label: "Sales & CRM", description: "Funnel, follow-ups, quotations and proposals.",
    sections: [
      {
        id: "funnel", label: "Funnel", description: "Stages, reasons, sources and activity types.",
        keywords: ["crm funnel"],
        settings: [
          s("crm.funnel.stage_probabilities", "Probability per deal stage", "Set when a deal enters the stage.", { columns: ["Stage", "Probability %"] }),
          s("crm.funnel.loss_reasons", "Loss reasons", "With the months until re-contact; empty months means never."),
          s("crm.funnel.disqualify_reasons", "Disqualification reasons", "With the months until re-contact."),
          s("crm.funnel.lead_sources", "Lead sources", "Keys already used on leads must stay."),
          s("crm.funnel.lead_priorities", "Lead priorities", "Keys already used on leads must stay."),
          s("crm.segments", "Segments", "The segments a lead, deal or catalog item can belong to, e.g. Retail or Public sector. Empty: segment is free text.", { columns: ["Segment"] }),
          s("crm.activity_types", "Activity types", "Keys already used on activities must stay."),
          s("crm.activity.max_chars", "Longest note", "Characters one note or activity may hold; a longer one is refused, never cut."),
          s("crm.funnel.default_lead_owner_email", "Owner of unassigned leads", "User email for leads that arrive with nobody behind them, e.g. from a web form.", { type: "email" }),
          s("crm.deal_dup.window_days", "Duplicate deal window (days)", "Two open deals of one contact or organization opened within this many days are checked as one deal; Nary asks you to merge them.", days),
          s("crm.dup.repropose_days", "Ask again about a duplicate after (days)", "A duplicate deal or lead pair whose merge card expired, failed or was withdrawn is proposed again after this many days. Only your \"not a duplicate\" hides a pair for good.", days),
        ],
      },
      {
        id: "crm-attributes", label: "CRM attributes", description: "Custom fields on leads, deals, organizations and contacts.",
        settings: [], block: "sales.crm-attributes", keywords: ["custom fields", "ai attributes"],
      },
      {
        id: "deleted-activities", label: "Deleted activities", description: "Restore an activity someone deleted (the Owner and admins).",
        settings: [], block: "sales.deleted-activities", keywords: ["deleted activities", "restore activity", "trash", "voided"],
      },
      {
        id: "follow-ups", label: "Follow-up reminders", description: "Reminder rules, delivery window and the weekly digest.",
        settings: [], block: "sales.followups", keywords: ["followups", "reminders", "digest"],
        blockFields: [
          { label: "Follow-up due hour", key: "crm.followup.due_hour" },
          { label: "Escalate after", key: "crm.followup.escalate_after_hours" },
          { label: "Default follow-up delay", key: "crm.followup.default_delay_days" },
          { label: "No reply after", key: "crm.followup.no_reply_days" },
          { label: "Idle lead after", key: "crm.followup.lead_idle_days" },
          { label: "Deal stuck in a stage after", key: "crm.followup.deal_stage_idle_days" },
          { label: "Unanswered quotation after", key: "crm.followup.quotation_followup_days" },
          { label: "Reminders delivered from / until", key: "crm.followup.quiet_from" },
          { label: "Weekly Owner digest", key: "crm.followup.digest_weekday" },
        ],
      },
      {
        id: "quotations", label: "Quotations", description: "Validity, expiry, discounts and the tax notes.",
        settings: [
          s("quotation_validity_days", "Quotation validity", "Days a new quotation stays valid.", days),
          s("sales.quotation.expire_grace_days", "Expire after validity ends", "Days before a sent quotation is marked expired; 0 is the day after.", days),
          s("sales.quotation.expired_nudge_days", "Re-quote reminder window", "Only quotations whose validity ended within this many days get a reminder; older ones expire quietly.", days),
          s("max_discount_pct_without_approval", "Discount without approval", "Largest discount on a quotation before it needs approval.", pct),
          // 2 Nov 2026: quotation tax notes (migration 0147)
          s("sales.quotation.tax_exclusive_note", "Prices exclude tax: the sentence", "What the \"Prices exclude tax\" switch (and tax none from your AI client or Nary) adds to the notes."),
          s("sales.quotation.tax_exclusive_phrases", "Notes that say prices exclude tax", "Phrases. Notes with one can't go to approval or be sent while a line carries tax.", { columns: ["Phrase"] }),
          s("sales.quotation.tax_inclusive_phrases", "Notes that say prices include tax", "Phrases. Notes with one can't go to approval or be sent while no line carries tax.", { columns: ["Phrase"] }),
        ],
        keywords: ["tax", "vat", "sales tax"],
      },
      {
        // 4 Nov 2026: the house notes of a quotation, a template per product family (migration 0158)
        id: "quotation-notes", label: "Quotation notes templates",
        description: "The house notes per product family: the ERP fills the house clauses, the deal clauses are written per quotation, and the check stops the approval when required clauses are missing.",
        settings: [
          s("sales.quotation.notes.block_submit", "Stop the approval when the notes fail the check", "Missing required clauses, notes not numbered, an accuracy figure, a delay penalty, or a discount from someone without the pricing permission. Sent or approved quotations are never re-checked."),
          s("sales.quotation.notes.lead_days_min", "Delivery from", "Working days after the order and payment are confirmed ({lead_days}).", days),
          s("sales.quotation.notes.lead_days_max", "Delivery until", "Working days after the order and payment are confirmed ({lead_days}).", days),
          s("sales.quotation.notes.invoice_due_days", "Invoices payable within", "Stated in Payment Terms.", days),
          s("sales.quotation.notes.non_payment_pause_days", "Service pauses after", "Days an invoice is overdue (subscriptions).", days),
          s("sales.quotation.notes.non_payment_terminate_days", "Service may end after", "Days an invoice is overdue (subscriptions).", days),
          s("sales.quotation.notes.warranty_years", "Hardware warranty", "Years from delivery.", { unit: "years" }),
          s("sales.quotation.notes_families", "Families and detection order", "order: the families, first match wins; fallback: the family used when none matches.", { type: "json" }),
          s("sales.quotation.notes_template.products", "Template: products", "detect (skus, categories, product_types, sale_models, keywords, subject_keywords, exclude_skus) and the ordered clauses {key, label, kind fixed|deal, required, when, wording_en, wording_en_foreign, extend, inline, guidance}; checked when saved.", { type: "json" }),
          s("sales.quotation.notes_template.services", "Template: services and projects", "As the products template; checked when saved.", { type: "json" }),
          s("sales.quotation.notes_template.subscription", "Template: subscriptions", "As the products template; checked when saved.", { type: "json" }),
          s("sales.quotation.notes.texts", "Payment and tax sentences", "Per language (en required; others optional, each complete): the payment, tax and currency-name sentences the fixed clauses fill in, and facts (travel SKUs, the services family, logistics keywords).", { type: "json" }),
          s("sales.quotation.notes.check_phrases", "What the notes check looks for", "discount: phrases that make a clause a discount, which only the Owner or the pricing permission may add.", { type: "json" }),
        ],
        keywords: ["notes", "terms", "template", "payment terms", "warranty", "care"],
      },
      {
        id: "pricing-rules", label: "Pricing rules",
        description: "What stops a quotation's approval (a price below the minimum multiple of cost) and what only warns. Only the Owner can let a blocked draft through, with a reason.",
        settings: [
          s("sales.pricing_rules.block_submit", "Stop the approval on a blocking pricing rule", "Off: the rules only warn. The Owner's override (with a reason) always lets a draft through."),
          s("sales.pricing_rules.min_cost_multiple", "Minimum multiple of full cost", "Against the catalog cost price; custom and service lines against the labour of the deal's cost sheet.", { unit: "x", step: 0.1 }),
          s("sales.pricing_rules.owner_approval_skus", "Always Owner approval", "SKU patterns: every quotation with one goes to the Owner. A proposed catalog price does the same.", { columns: ["SKU pattern"] }),
        ],
        keywords: ["pricing", "margin", "cost", "rules", "override"],
      },
      {
        id: "pricing", label: "Pricing & payment terms", description: "Catalog currencies, split payments and margins.",
        settings: [
          s("catalog.conversion_base_currency", "Catalog base currency", "Other prices are converted from it, e.g. USD."),
          s("catalog.independent_currencies", "Currencies with their own price list", "Never converted, e.g. USD and JPY."),
          s("sales.split_payment_min_unit_equivalents", "Split payment from", "Unit-equivalents from which product quotations suggest a split payment."),
          s("sales.split_payment_on_order_pct", "Paid on order under a split payment", undefined, pct),
          s("sales.service_down_payment_pct", "Down payment on services", "Suggested on service quotations.", pct),
          s("sales.margins", "Target and floor margins", "Gross margin % per product category and its catalog families (internal)."),
        ],
      },
      {
        id: "proposals", label: "Proposals", description: "Template, wording, discovery checklist and figures.",
        settings: [
          s("documents.proposal.template", "Proposal sections", "Headings per language, first blocks, and what Nary writes in each."),
          s("documents.proposal.labels", "Fixed words per language", "Cover, coverage statuses, stamp, header."),
          s("documents.proposal.validity_days", "Proposal validity", undefined, days),
          s("documents.proposal.classification", "Classification line", "Per language."),
          s("documents.proposal.prepared_by", "Prepared by line", "Per language."),
          s("proposal.discovery.fields", "Discovery checklist", "What Nary asks, one or two at a time."),
          s("proposal.discovery.min_coverage_pct", "Checklist known before drafting", "The rest become assumptions.", pct),
          // visuals V4
          s("proposal.visuals.default_figures", "Figures in every new draft", "Charts and diagrams Nary always adds: timeline, architecture.", { columns: ["Figure"] }),
          s("proposal.visuals.vision_check", "Check each figure by eye", "Nary looks at the drawn chart or diagram for overlaps, missing items and wrong flow."),
          s("proposal.visuals.max_attempts", "Figure fixes before telling you", "Failed checks Nary fixes on its own before the figure is kept with its warnings and you are told."),
          s("proposal.visuals.custom_svg_max_kb", "Largest custom drawing", "Size limit for a chart or diagram Nary or the MCP client draws itself.", { unit: "KB" }),
          // 1 Nov 2026: direct proposal authoring (migration 0146)
          s("proposal.required_figures_by_requirement", "Figures the requirements make required", "Requirement sheet flags and the figure each one makes required (network, process). Timeline and architecture are always required. Approval is refused while one is missing or failed.", { columns: ["Requirement flag", "Figure"] }),
          s("proposal.price_text_sections", "Sections that may state prices", "Section keys whose text may show money, written only by the Owner or someone with the discount permission (logged). Elsewhere prices come from the quotations.", { columns: ["Section key"] }),
        ],
        keywords: ["chart", "diagram", "figure", "timeline", "architecture"],
      },
      {
        id: "cost-breakdown", label: "Cost breakdown & rate card", description: "How a proposal's man-day costs are worked out.",
        block: "sales.rate-card", keywords: ["rate card", "man-day"],
        blockFields: [{ label: "Rate card", hint: "Cost per man-day by role and seniority (internal)." }],
        settings: [
          s("proposal.cost.contingency_pct", "Contingency", "On labour and non-labour.", pct),
          s("proposal.cost.days_per_month", "Working days per month", "Turns a monthly salary into a man-day."),
          s("proposal.cost.load_factor", "Employer load factor", "Employer costs on top of a gross salary (contributions, bonuses), for a man-day cost."),
          s("proposal.cost.min_monthly", "Minimum monthly wage", "The floor of any rate (0 = none). Update each year."),
          s("proposal.cost.update_threshold_pct", "Propose a rate change above", "Difference Nary needs before it proposes a new rate.", pct),
        ],
      },
      {
        id: "partners", label: "Partners", description: "Lead routing to partners and deal registration.",
        settings: [
          s("nary.partners.routing", "Partner routing rules", "A lead matching region and industry is proposed to that partner."),
          s("nary.partners.warn_days", "Warn before protection ends", "Days before a deal registration's protection ends.", days),
          s("partner.training.max_attempts", "Training quiz attempts", "Quiz submissions a partner user gets per training module."),
        ],
      },
      {
        id: "scheduled", label: "Scheduled messages", description: "Messages queued to send later.",
        keywords: ["automation"],
        settings: [
          s("crm.scheduled.max_attempts", "Attempts before failing"),
          s("crm.scheduled.stuck_after_minutes", "Fail a send stuck for", undefined, minutes),
        ],
      },
    ],
  },
  {
    id: "money", label: "Money & tax", description: "Currency, tax, invoices and accounting.",
    sections: [
      {
        id: "currency", label: "Currencies", description: "Functional currency, active currencies and exchange rates.",
        settings: [], block: "money.currency", keywords: ["exchange rate", "fx"],
        blockFields: [
          { label: "Functional currency", hint: "Base currency for P&L, balance sheet and tax reports.", key: "currency.functional" },
          { label: "Active currencies", hint: "Only these appear in transaction dropdowns.", key: "currency.active" },
          { label: "ExchangeRate API key", key: "currency.api_key" },
          { label: "Exchange rate sync" },
        ],
      },
      {
        id: "accounting", label: "Accounting", description: "Ledger posting, decimals and default sales tax.",
        keywords: ["finance", "ledger"],
        settings: [
          s("finance.gl_autopost_from", "Post to the ledger automatically from", "Blank is off. Map accounts and link bank accounts first.", { type: "date" }),
          s("currency.decimals", "Decimal places per currency", "e.g. JPY 0, USD 2.", { columns: ["Currency", "Decimals"] }),
          s("finance.default_sales_tax", "Default sales tax by currency", "Tax codes per currency, e.g. EUR VAT20.", { columns: ["Currency", "Tax code"] }),
        ],
      },
      {
        id: "invoices", label: "Invoices", description: "Due dates and overdue reminders.",
        settings: [
          s("invoice_due_days", "Invoice due after", undefined, days),
          s("finance.invoice_reminder_days", "Overdue reminders", "Days after the due date, e.g. 1, 7, 14, 30.", { columns: ["Days after due"] }),
          s("finance.invoice_reminder_repeat_days", "Then remind every", "0 stops after the list above.", days),
        ],
      },
      {
        id: "taxes", label: "Taxes", description: "Tax configurations and rules live in Finance.",
        settings: [], block: "money.taxes", keywords: ["vat", "sales tax", "tax rules"],
      },
    ],
  },
  {
    id: "people", label: "People & payroll", description: "Attendance, payroll, tax and overtime.",
    sections: [
      {
        id: "attendance", label: "Attendance", description: "Face match, location precision and working hours.",
        keywords: ["hr", "face recognition"],
        settings: [
          s("hr.face_match_distance", "Face match strictness", "Largest face distance accepted: 0.35 strict to 0.60 lenient. Slightly weaker matches go to HR review.", { min: 0.35, max: 0.6, step: 0.01 }),
          s("hr.geo_max_accuracy_m", "Largest GPS inaccuracy", "Personal-device clock-ins with a less precise location are refused.", { unit: "m", min: 10, max: 1000, step: 10 }),
          s("hr.work_start_time", "Work start time", "Clock-ins after this time are flagged late.", { type: "time" }),
          s("hr.overnight_shift_hours", "Overnight shift window", "A clock-out after midnight closes yesterday's shift if it began within this many hours.", { ...hours, min: 1, max: 24, step: 1 }),
          s("hr.face.step_timeouts_ms", "Face check time per step", "Milliseconds for each step of the head-turn check, counted from when the step begins: find (find the face and hold still), action (turn the head), return (face the camera again). 1000 to 60000 each.", { type: "json" }),
          s("hr.face.challenge_ttl_seconds", "Phone check valid for", "How long a phone's face check stays valid. Never shorter than the three steps together plus 15 s for the upload.", { ...seconds, min: 10, max: 600, step: 5 }),
          s("hr.face.passive.enabled", "Passive face check first", "The screen flashes a few soft colours while the camera looks; a live face reflects them. When the check is not sure, the head turn follows. Off: always the head turn."),
          s("hr.face.passive.flash_count", "Colours per check", "How many colours the screen flashes, 4 to 6.", { min: 4, max: 6, step: 1 }),
          s("hr.face.passive.flash_ms", "Time per colour", "At least 250 ms, so the screen flashes fewer than 3 times a second (photosensitivity safe).", { unit: "ms", min: 250, max: 2000, step: 10 }),
          s("hr.face.passive.settle_ms", "Photo after", "When the photo is taken after each colour comes on. Must be shorter than the time per colour; late is better, because phone cameras deliver their frames 100 to 250 ms late (default: 60 ms before the colour ends).", { unit: "ms", min: 0, max: 1999, step: 10 }),
          s("hr.face.passive.palette", "Flash colours", "The soft colours the check picks from, as [r, g, b] lists (0 to 255), at least 4. No colour is used twice in a row.", { type: "json" }),
          s("hr.face.passive.flash_min_delta", "Smallest colour response", "How much the face colour must change, in chromaticity units (each channel's share of the brightness, times 255), so a camera's auto exposure does not count. Below it (for example in bright window light) the check is not sure and the head turn follows.", { min: 0.5, max: 60, step: 0.5 }),
          s("hr.face.passive.flash_min_corr", "Colour match needed", "How closely the face colour must follow the flashed colours, 0 to 1 (a camera one colour late is allowed for). Below it the check fails and the head turn follows.", { min: 0, max: 1, step: 0.05 }),
          s("hr.face.passive.spoof_min_real", "Real face score needed", "Smallest score of the anti-spoof model that the face is real and not a photo or screen, 0 to 1.", { min: 0, max: 1, step: 0.05 }),
        ],
      },
      {
        id: "offices", label: "Office locations", description: "Geofence centres: employees must be inside the radius to clock in.",
        settings: [], block: "people.offices", keywords: ["geofence"],
      },
      {
        id: "kiosks", label: "Kiosk tablets", description: "Dedicated clock-in tablets.",
        settings: [], block: "people.kiosks",
      },
      {
        id: "payroll", label: "Payroll", description: "Contributions and income tax. Payroll refuses to run while either is empty or malformed.",
        keywords: ["contributions", "social security", "pension", "health insurance", "income tax", "withholding", "brackets"],
        settings: [
          s("payroll.contributions", "Contributions", "Each one: key, label, employee_rate and employer_rate (% of the fixed wage), base_cap (the monthly wage they apply to at most, or null), employer_taxable (the employer share counts as income), employee_deductible (the employee share lowers taxable income). [] = none.", { type: "json", required: true }),
          s("payroll.income_tax", "Income tax", "{\"method\": \"none\"} or {\"method\": \"progressive\", \"brackets\": [[annual upper bound, rate %], ..., [null, rate %]], \"allowances\": {status: yearly tax-free amount}, \"default_status\", \"expense_deduction_rate\", \"expense_deduction_monthly_cap\", \"taxable_rounding\"}. Withheld monthly on the cumulative method; the last month settles the year.", { type: "json", required: true }),
        ],
      },
      {
        id: "overtime", label: "Overtime", description: "Hourly wage and multiples.",
        settings: [
          s("payroll.overtime_hours_divisor", "Hourly wage divisor", "Hourly wage is the monthly fixed wage divided by this (e.g. 173).", { required: true }),
          s("payroll.overtime_multipliers", "Overtime multiples", "Per day type: cumulative hour bound and the multiple of the hourly wage.", { type: "json", required: true }),
          s("payroll.rest_weekdays", "Rest days", "Weekdays, Monday 0 to Sunday 6; days marked holiday in attendance also count.", { type: "json", required: true, columns: ["Weekday"] }),
        ],
      },
    ],
  },
  {
    id: "nary", label: "Nary & AI", description: "How Nary works with customers and with you.",
    sections: [
      {
        id: "ai-features", label: "AI features", description: "The master switch and what the AI may do.",
        settings: [], block: "ai.general", keywords: ["ai", "em dash", "test connection"],
        blockFields: [
          { label: "AI features", hint: "Off stops every AI feature, Nary included." },
          { label: "No em dashes in anything the AI writes" },
          { label: "Nary chat" }, { label: "Draft quotations and invoices" }, { label: "CRM lead scoring" },
          { label: "Reply and draft suggestions" }, { label: "Document intelligence" },
        ],
      },
      {
        id: "providers", label: "Providers & keys", description: "API keys of the AI providers.",
        settings: [], block: "ai.keys", keywords: ["api keys", "anthropic", "openai", "gemini", "deepseek"],
        blockFields: [{ label: "AI provider API keys", key: "ai.anthropic.api_key" }],
      },
      {
        id: "models", label: "Model per job", description: "Which model each AI job uses.",
        settings: [], block: "ai.routing", keywords: ["routing"], blockFields: [{ label: "AI routing", key: "ai.routing" }],
      },
      {
        id: "budget", label: "Budget", description: "What the AI may spend each month.",
        settings: [], block: "ai.budget", keywords: ["cost", "usage", "pricing"],
        blockFields: [
          { label: "Monthly AI budget", key: "ai.budget.monthly_usd" },
          { label: "Warn at", key: "ai.budget.warn_pct" },
          { label: "Stop non-essential jobs at", key: "ai.budget.stop_pct" },
          { label: "Stop every job at", key: "ai.budget.hard_stop_pct" },
          { label: "Essential jobs", key: "ai.budget.essential_jobs" },
          { label: "Cap per job", key: "ai.budget.job_caps" },
          { label: "Model prices", key: "ai.pricing" },
          { label: "Batch discount", key: "ai.pricing.batch_discount_pct" },
        ],
      },
      {
        id: "persona", label: "Chat persona", description: "How Nary thinks and advises when staff chat with it.",
        settings: [], block: "ai.persona", keywords: ["system prompt", "assistant"],
      },
      {
        id: "chat", label: "AI chat", description: "What the chat knows about the ERP and the catalog.",
        settings: [
          s("ai.assistant.catalog_digest_max_items", "Products in the catalog overview", "Above this, families only."),
          s("ai.erp_guide.enabled", "ERP guide in AI Knowledge", "Keep the guide to how the ERP works, built from the code."),
        ],
      },
      {
        id: "research", label: "Web research", description: "Where research and enrichment get their facts.",
        settings: [], block: "ai.research", keywords: ["searxng", "tavily", "brave", "search"],
      },
      {
        id: "enrichment", label: "Company enrichment", description: "What enrichment may fill in by itself.",
        settings: [], block: "ai.enrichment",
        blockFields: [
          { label: "Research new companies", key: "crm.enrich.on_create" },
          { label: "Fields enrichment may fill", key: "crm.enrich.fields" },
          { label: "Minimum confidence", key: "crm.enrich.min_confidence" },
          { label: "Re-research after", key: "crm.enrich.refresh_days" },
          { label: "Daily limit", key: "crm.enrich.daily_limit" },
        ],
      },
      {
        id: "scoring-rubric", label: "Lead-scoring rubric", description: "How the AI scores leads hot, warm or cold.",
        settings: [], block: "ai.scoring",
      },
      {
        id: "knowledge", label: "AI knowledge", description: "Documents only the AI reads, and the knowledge index.",
        settings: [], block: "ai.knowledge", keywords: ["ai knowledge", "reindex", "sop"],
      },
      {
        id: "nary-schedule", label: "Nary's day", description: "Working hours, briefs and what interrupts you.",
        keywords: ["nary"],
        settings: [
          s("nary.working_hours", "Working hours", "Company time; days 1 is Monday."),
          s("nary.heartbeat_minutes", "Check the to-do list every", "Inside working hours.", minutes),
          s("nary.brief_times", "Brief times", "Morning, midday and wrap-up; empty is off."),
          s("nary.urgent", "What interrupts you", "Hot score, customer wait minutes, segments, events."),
          s("nary.briefs.items_per_section", "Items per brief section on WhatsApp", "Reply \"more\" for the rest; the email has all."),
          s("nary.briefs.snooze_days", "Snooze on a follow-up", "\"snooze N\" on an overdue follow-up pushes it out by this many days.", days),
        ],
      },
      {
        id: "autonomy", label: "Autonomy & approvals", description: "When Nary may act without asking.",
        settings: [
          s("nary.autonomy.promotion_count", "Approvals before promotion", "Approvals in a row without a meaningful edit before an action can be promoted."),
          s("nary.autonomy.edit_threshold_pct", "Meaningful edit above", "Share of changed text.", pct),
          s("nary.autonomy.demotion_floor", "Lowest automatic demotion", "1 is ask."),
          s("nary.pending.expire_days", "Undecided approval cards expire after", undefined, days),
          s("nary.apply_wait_seconds", "Wait for long work before answering", "Drafting or applying a card that takes longer answers \"still working\" after this and finishes on its own. Keep it below the proxy's 60 s.", seconds),
          s("nary.applying_stale_minutes", "A card still applying is failed after", "With nothing running any more (the connection was cut); you are told and can try again.", minutes),
          s("nary.shadow_exempt_actions", "Sent even in shadow mode", "Customer actions that still go out (the acknowledgement)."),
        ],
      },
      {
        id: "replies", label: "Replies & pacing", description: "How fast and how often Nary writes to a customer.",
        settings: [
          s("nary.reply.debounce_seconds", "Wait before replying", "After a customer's message, so a burst of messages gets one reply; 0 answers at once.", seconds),
          s("nary.reply.backfill_window_hours", "No automatic reply to late finds older than", "A message found late, or older than our last message in the chat, never gets an automatic reply.", hours),
          s("nary.wa.chat_max_messages", "Messages per chat before asking you", "Within the window below; more needs your approval."),
          s("nary.wa.chat_window_minutes", "Per-chat limit window", undefined, minutes),
          s("nary.wa.burst_alert_messages", "Burst alert above", "Alert when a chat gets more of our messages within the window below."),
          s("nary.wa.burst_alert_minutes", "Burst alert window", undefined, minutes),
          s("nary.wa.complaint_pause_hours", "Pause a chat after a complaint", "When a customer says they get repeated messages, automatic sends need your approval this long.", hours),
          s("nary.reply.unanswered_business_hours", "Unanswered chat after", "The daily check lists chats where the customer has had the last word this many working hours.", hours),
          s("nary.reply.unanswered_lookback_days", "Unanswered chat check looks back", undefined, days),
          s("nary.wa.greeting_hours", "Greeting hours", "Company-time hour each greeting starts (Good morning, afternoon, evening), decided at send time.", { columns: ["Greeting", "From hour"] }),
          s("nary.reply.questions", "Qualifying questions", "Up to two per product interest, per language."),
          s("nary.reply.max_questions", "Most questions in one message"),
          s("nary.reply.max_lines", "Most lines in one WhatsApp bubble"),
          s("nary.price.catalog_terms", "Catalog search words per product", "Used for price answers."),
          s("nary.writing.lint_enabled", "Writing check", "Every customer message is checked before its card (form of address, time greeting, Google Meet only, em dashes, unconfirmed promises); one rewrite, else the problems show on the card."),
          s("nary.writing.meeting_offer_patterns", "Call or meeting offer patterns", "Nary's own follow-ups never offer a call or meeting unless the customer asked. A follow-up matching one of these patterns is rewritten."),
        ],
      },
      {
        id: "whatsapp-channel", label: "Nary on WhatsApp", description: "Sign-off, sending limits and health checks.",
        settings: [
          s("nary.wa.signature", "Nary's sign-off", "Added under every message Nary sends."),
          s("nary.wa.max_per_hour", "Most messages in an hour", "Ban risk."),
          s("nary.wa.max_per_day", "Most messages in a day", "Ban risk."),
          s("nary.wa.takeover_hours", "Stay silent after a person replies", "From the phone, in that chat.", hours),
          s("nary.wa.gapcheck_minutes", "Look for missed messages every", undefined, minutes),
          s("nary.wa.gapcheck_lookback_hours", "Missed-message check covers chats active in the last", undefined, hours),
          s("nary.wa.gapcheck_grace_seconds", "Leave messages younger than this to the live feed", undefined, seconds),
          s("nary.wa.health_minutes", "Check the link and the queue every", undefined, minutes),
          s("nary.wa.health_backlog_minutes", "Stuck after", "An event waiting longer means Nary is stuck; the Owner is emailed.", minutes),
          s("nary.wa.media_max_mb", "Largest attachment stored", undefined, { unit: "MB" }),
          s("nary.wa.card_notices", "Approval cards on your WhatsApp", "In working hours: reply 1 apply, 2 view, 3 skip."),
          s("nary.wa.labels_enabled", "Keep WhatsApp Business labels in step with the funnel"),
          s("nary.wa.label_map", "Stage to WhatsApp label", "Deal stage, or lead:<status>, to an existing WhatsApp Business label.", { columns: ["Stage", "Label"] }),
          s("nary.stt.max_seconds", "Longest voice note transcribed", undefined, seconds),
        ],
      },
      {
        id: "intake", label: "Intake & acknowledgement", description: "First contact: the automatic reply and web forms.",
        block: "nary.ack-templates", keywords: ["web3forms", "ack"],
        blockFields: [{ label: "Acknowledgement texts", hint: "Per language: in hours, after hours, late prefix.", key: "nary.ack.templates" }],
        settings: [
          s("nary.ack.enabled", "Send the automatic acknowledgement", "Turn it on together with turning off WhatsApp's own greeting."),
          s("nary.ack.default_language", "Acknowledgement language", "When the customer's cannot be told.", { type: "select", options: [{ value: "en", label: "English" }] }),
          s("nary.ack.ongoing_days", "A chat is ongoing within", "No acknowledgement for a chat with a message in the last N days.", days),
          s("nary.ack.late_after_minutes", "Apologise when later than", "The acknowledgement starts with the apology.", minutes),
          s("nary.intake.arrival_window_minutes", "Wait for a promised WhatsApp chat", "After a [whatsapp] web form, then tell the Owner.", minutes),
          s("nary.intake.currency_by_country", "Quoting currency by country", "ISO code; * is everyone else.", { columns: ["Country", "Currency"] }),
          s("nary.web3forms.sender_domains", "Form email sender domains", "Of the Web3forms notification emails."),
          s("nary.web3forms.dkim_domains", "Required DKIM domains", "A signature from one of these is required."),
          s("nary.web3forms.known_fields", "Known form fields", "Another field gives a format-drift warning, still stored."),
          s("nary.web3forms.required_fields", "Required form fields", "Missing ones mean AI extraction and a warning."),
          s("nary.web3forms.routes", "Form journeys", "submission_source to journey (email: reply by email; whatsapp: expect the chat).", { columns: ["Source", "Journey"] }),
          s("nary.web3forms.interest_map", "Interest to product family", "A hint; Nary reads the message.", { columns: ["Interest code", "Product family"] }),
          s("nary.web3forms.poll_junk", "Also read form emails in Junk"),
        ],
      },
      {
        id: "contact-matching", label: "Identity & triage", description: "Matching a chat to a contact, and spam triage.",
        settings: [
          s("nary.identity.default_country", "Default phone country", "For numbers written without a country code (ISO)."),
          s("nary.identity.prefill_labels", "Pre-filled WhatsApp text labels", "Label in the website's text to field.", { columns: ["Label", "Field"] }),
          s("nary.identity.thresholds", "Match thresholds", "Score from which a match is certain (auto-link), likely (link and tell), weak (ask)."),
          s("nary.identity.signal_scores", "Signal scores", "Score of each matching signal; the best one counts."),
          s("nary.identity.honorifics", "Ignored honorifics", "When comparing names."),
          s("nary.identity.recent_form_days", "Match a web form sent within", undefined, days),
          s("nary.triage.example_count", "Corrections shown as examples", "The Owner's latest triage corrections."),
          s("nary.triage.business_signals", "Business words", "Send a spam or personal verdict to the second model for review."),
        ],
      },
      {
        id: "scoring", label: "Lead scoring (BANT)", description: "Nary's qualification rubric.",
        settings: [
          s("nary.score.rubric", "BANT rubric", "Points per answer."),
          s("nary.score.classes", "Score classes", "Class to lowest and highest total (0 to 12)."),
          s("nary.score.boundary_margin", "Re-score near a boundary", "A total this close is re-scored by the second model."),
          s("nary.score.red_flags", "Red flags", "Fit: consider disqualifying."),
          s("nary.score.green_flags", "Green flags", "Fit."),
        ],
      },
      {
        id: "follow-up-cadence", label: "Follow-up cadence", description: "After a quotation is sent.",
        settings: [
          s("nary.cadence.steps", "Steps", "Day, channel, intent, topic, news."),
          s("nary.autosend.min_gap_hours", "Hours between automatic messages", "Nary sends one customer at most one automatic message (follow-up, re-engagement, lost-deal reply) in this many hours; another one waits for you.", hours),
          s("nary.cadence.min_gap_days", "Never the same topic within", undefined, days),
          s("nary.cadence.max_without_news", "Most follow-ups without news"),
          s("nary.cadence.at_risk_silent_days", "At risk when silent", "No answer this many days before expiry.", days),
          s("nary.cadence.nurture_silent_days", "Propose nurture when silent", "This long after expiry.", days),
          s("nary.cadence.recontact_days", "Re-engage after nurture", undefined, days),
        ],
      },
      {
        id: "negotiation", label: "Negotiation & contract", description: "Objections, competitors and closing.",
        settings: [
          s("nary.negotiation.scripts", "Value-first answers per objection", "Owner only."),
          s("nary.negotiation.competitor_checklist", "Competitor comparison checklist", "What to compare like for like."),
          s("nary.contract.nudge_days", "Remind about an unsigned contract every", undefined, days),
          s("nary.contract.won_needs_dp", "Won needs contract and down payment", "The deal moves to Won only when the contract is signed and the down payment paid."),
          s("nary.handoff.kickoff_tasks", "Kickoff tasks on the new project", "Tasks created on the project when a won deal is handed off."),
        ],
      },
      {
        id: "email", label: "Nary and email", description: "Which mail Nary reads and how it signs.",
        settings: [
          s("nary.email.intake_enabled", "Read new CRM inbox mail", "Mail not from an open lead or deal: triage, lead, acknowledgement, reply."),
          s("nary.email.reply_window_hours", "Only reply to mail from the last", "Older mail is history.", hours),
          s("nary.email.skip_rules", "Skipped before any AI call", "List headers, precedence, auto-submitted, sender prefixes and domains."),
          s("nary.email.signature_fields", "Nary's email signature values", "Name, title, email, mobile: whatever the template uses."),
        ],
      },
      {
        id: "meetings", label: "Meetings", description: "Booking, reminders and recordings.",
        keywords: ["google calendar"],
        settings: [
          s("nary.google.account_email", "Google Calendar account", "Meetings are booked in it.", { type: "email" }),
          s("nary.google.calendar_id", "Calendar ID"),
          s("nary.meetings.rules", "Booking rules", "Length, buffer, per day, start window (company time), blocks, slots offered, horizon, reduced-hours start window."),
          s("nary.meetings.holidays", "Public holidays", "No meetings. Update each year."),
          s("nary.meetings.reduced_hours_periods", "Reduced-hours periods", "Date ranges when the shorter start window applies."),
          s("nary.meetings.customer_hours", "Customer working week by country", "ISO; * is the default; days 1 is Monday."),
          s("nary.meetings.text", "Texts sent to the customer", "Slots, reminders, recording notice, per language."),
          s("nary.meetings.reminders", "Reminder times", "Day before at a time, minutes before, and when a missing customer is a no-show."),
          s("nary.meetings.recording_max_minutes", "Longest recording transcribed", undefined, minutes),
          s("nary.meetings.recording_max_mb", "Largest recording file", undefined, { unit: "MB" }),
          s("nary.meetings.notes_max_mb", "Largest notes or transcript file", undefined, { unit: "MB" }),
          s("nary.meetings.recording_poll_seconds", "Check a transcription every", undefined, seconds),
          s("nary.meetings.recording_deadline_hours", "Give up on a transcription after", undefined, hours),
          s("nary.meetings.recording_resubmits", "Resend a lost recording", "Times it is sent to speech-to-text again."),
          s("nary.meetings.find_max_days", "Find free slots up to", "How far ahead find_meeting_slots (the MCP client, Nary's chat) may look.", days),
          s("nary.meetings.find_max_slots", "Free slots listed at most", "The most slots one find_meeting_slots answer lists."),
        ],
      },
      {
        id: "nary-and-you", label: "Nary and you on WhatsApp", description: "How Nary talks to the Owner.",
        keywords: ["owner", "reply alerts"],
        settings: [
          s("nary.owner.enabled", "Nary writes to you on WhatsApp"),
          s("nary.owner.one_at_a_time", "One topic at a time", "The next comes after you answer or say \"next\"; urgent ones still go at once."),
          s("nary.owner.urgent_kinds", "Urgent kinds"),
          s("nary.owner.bundle_minutes", "Bundle topics within", undefined, minutes),
          s("nary.owner.remind_working_hours", "Reminders only in working hours"),
          s("nary.owner.default_language", "Language"),
          s("nary.owner.consult_quotations", "Consult you on quotations"),
          s("nary.owner.consult_proposals", "Consult you on proposals"),
          s("nary.owner.topics_in_context", "Topics kept in context"),
          s("nary.owner.quiet_minutes", "Wait until you are quiet for", "A new topic waits; urgent ones do not.", minutes),
          s("nary.owner.alert_stale_hours", "Drop alerts older than", "An alert still unsent is old news.", hours),
          s("nary.owner.confirm_twice_actions", "Confirm twice", "Decisions that touch money, confirmed again on their final version."),
          s("nary.reply_alerts.enabled", "Reply alerts", "Tell you when a customer answers one of our messages."),
          s("nary.reply_alerts.mode", "Reply alert mode", undefined, { type: "select", options: [{ value: "action_needed", label: "Only replies that need us (rest in the brief)" }, { value: "every_reply", label: "Every reply" }] }),
          s("nary.send_at.max_days", "Latest scheduled send", "A message you tell Nary to send later may be at most this far ahead.", days),
          s("nary.attach.max_files", "Files per customer message", "Documents and files one message from Nary may carry."),
          s("nary.attach.max_total_mb", "Total size of a message's files", "Larger mail is refused by many servers: on email a stored file that does not fit goes as a download link. WhatsApp videos have their own limit.", { unit: "MB" }),
          s("nary.email.link_days", "Email download links work for", "A file too large for the email goes as a link; at most 7 days.", days),
          s("nary.upload.max_mb", "Largest file your AI client or Nary uploads", "A file put on a lead, deal, contact or organization by upload_file or a saved preview (images, PDF, Word, Excel).", { unit: "MB" }),
          s("nary.upload.video_max_mb", "Largest video your AI client or Nary uploads", "MP4, MOV or WebM, not converted. Also the largest video sent on WhatsApp (as a document).", { unit: "MB" }),
          s("nary.wa.video_max_mb", "WhatsApp plays videos up to", "An MP4 up to this size goes as a video that plays in the chat; a larger one goes as a document.", { unit: "MB" }),
          s("nary.wa.media_timeout_seconds", "Wait for one WhatsApp file", "How long the ERP waits while a file (a large video takes minutes) is sent.", seconds),
          s("files.upload_link.minutes", "Upload links work for", "A link your AI client or Nary makes (create_upload_link) so you can drop files from a chat into the ERP.", minutes),
          s("files.upload_link.max_files", "Files per upload link", "The most files one upload link takes; ask for a new link for more."),
          s("files.upload_link.requests_per_minute", "Upload link requests per minute", "Per address and per link, on the drop page and its upload address."),
          s("nary.files.max_mb", "Largest customer file Nary reads", "PDFs, photos, Word and Excel files from chats, emails and records.", { unit: "MB" }),
          s("nary.files.vision_min_chars", "Scanned page threshold", "A PDF page with fewer characters of text than this is read by the vision model."),
          s("nary.files.vision_max_pages", "Most pages read by vision per file", "Scanned pages and photos cost AI; the rest of a long scan is left out."),
          s("nary.files.page_cap", "Most PDF pages read per file"),
          s("nary.files.sheet_max_rows", "Rows kept per spreadsheet sheet"),
          s("nary.files.read_max_chars", "Text per read", "How much of a file one read gives Nary (characters); it pages through the rest."),
          s("nary.report.kpi_targets", "Weekly report KPI targets", "SOP KPI targets."),
          s("nary.report.weekday", "Weekly report day", "Comes with the morning brief; 1 is Monday."),
          s("nary.report.window_days", "Weekly report covers", "Conversion KPIs over the last N days.", days),
          s("nary.report.response_sla_minutes", "Response SLA", "A first customer message answered within this meets it.", minutes),
          s("nary.sop_revisions.briefs", "SOP revision briefs", "What each SOP draft must change."),
          s("nary.sop_revisions.codes", "SOP revision codes", "related: SOP codes Nary reads for the house format; new: the code of a new SOP it may draft (empty: none)."),
        ],
      },
      {
        id: "staff-chat", label: "Nary chat on WhatsApp", description: "Staff chatting with Nary from their phones.",
        settings: [
          s("nary.staff_chat.enabled", "Staff can chat with Nary on WhatsApp"),
          s("nary.staff_chat.voice", "Voice notes"),
          s("nary.staff_chat.session_hours", "Session length", undefined, hours),
          s("nary.staff_chat.max_lines", "Most lines in one message", "The rest waits for \"next\"."),
          s("nary.staff_chat.max_bubbles", "Most bubbles per answer"),
          s("nary.staff_chat.bubble_chars", "Characters per bubble"),
          s("nary.staff_chat.wait_seconds", "Wait for the previous answer", "A message waits up to this long for the answer to the one before.", seconds),
          s("nary.staff_chat.late_reply_hours", "Answer missed messages within", "Marked late.", hours),
          s("nary.staff_chat.card_window_minutes", "Answer a card list by number within", "After Nary wrote something else.", minutes),
          s("nary.mailboxes.notify_owner", "Tell owners when Nary reads their mailbox", "When someone else's Nary read it."),
        ],
      },
      {
        id: "learning", label: "Skills & style", description: "What Nary learns from you.",
        settings: [
          s("nary.skills.max_in_prompt", "Skills listed in the chat prompt", "The rest are found with use_skill."),
          s("nary.skills.tool_ideas_project", "Tool ideas project", "The R&D project where Nary files ideas for tools it lacks."),
          s("nary.style.refresh_days", "Re-learn your style every", "WhatsApp and email style, proposed for approval.", days),
        ],
      },
      {
        id: "history", label: "History import", description: "Reading old WhatsApp chats.",
        settings: [
          s("nary.history.full_read_months", "Read in full the last", "Older chats are summarised.", { unit: "months" }),
          s("nary.history.chats_per_run", "Chats per run", "Ban risk."),
          s("nary.history.page_size", "Messages per request"),
          s("nary.history.pause_seconds", "Pause between requests", "Ban risk.", seconds),
          s("nary.history.run_minutes", "Run every", undefined, minutes),
          s("nary.history.pages_per_run", "Pages per chat per run"),
          s("nary.history.batch_size", "Chats per AI batch"),
          s("nary.history.sync_attempts", "Requests for older messages", "Before a chat counts as complete."),
          s("nary.history.style_sample", "Replies read to learn your style"),
          s("nary.history.missed_after_days", "Missed opportunity after", "A business chat left unanswered this long.", days),
        ],
      },
      {
        id: "engine", label: "Engine", description: "Events and error cards.",
        settings: [
          s("nary.events.batch_size", "Events per minute", "Handed to Nary's subscribers."),
          s("nary.events.max_attempts", "Attempts before an event is parked"),
          s("nary.events.resume_max_age_hours", "On resume, skip events that waited longer than",
            "While Nary is paused, website, form and email leads still come in. Its own work on events (replies, follow-ups, cards) waits; when Nary is resumed, events older than this are marked skipped instead of acted on late.", hours),
          s("nary.errors.dedupe_hours", "Repeat an error card at most every", undefined, hours),
        ],
      },
      {
        id: "work-queue", label: "Work queue", description: "What waits while Nary is paused, and your WhatsApp nudge.",
        keywords: ["queue", "nudge", "mcp", "paused", "waiting", "urgent"],
        settings: [
          s("nary.queue.nudge_enabled", "Nudge me on WhatsApp", "\"N items waiting\" with the top items. It runs while Nary is paused."),
          s("nary.queue.nudge_times", "Nudge times", "Company time, HH:MM, on working days (Nary's working days, not the meeting holidays)."),
          s("nary.queue.urgent_kinds", "Urgent kinds", "Nudged at once within working hours, e.g. whatsapp_reply, email_reply, new_lead, unknown_chat."),
          s("nary.queue.urgent_gap_minutes", "Urgent nudges at most every", undefined, minutes),
          s("nary.queue.today_kinds", "Kinds for today", "Listed after the urgent ones."),
          s("nary.queue.nudge_items", "Items listed in a nudge"),
          s("nary.queue.default_limit", "Items an AI client gets by default"),
          s("nary.queue.quotation_expiry_days", "Sent quotations listed before they expire", undefined, days),
          s("nary.queue.meeting_reminder_hours", "Meetings without a reminder, starting within", undefined, hours),
          s("nary.queue.meeting_brief_hours", "Meetings without your brief, starting within", undefined, hours),
          s("nary.queue.card_expiry_hours", "Cards listed as expiring within", "Of \"Undecided approval cards expire after\".", hours),
          s("nary.queue.new_lead_days", "New and triaging leads listed for", undefined, days),
        ],
      },
    ],
  },
  {
    id: "messaging", label: "Email & WhatsApp", description: "Mail servers, signatures and WhatsApp.",
    sections: [
      {
        id: "signature", label: "Email signature", description: "The layout every staff signature uses.",
        settings: [], block: "messaging.signature-template", keywords: ["email"],
      },
      {
        id: "test-address", label: "Test recipient", description: "Where the Send test buttons below send to.",
        settings: [], block: "messaging.test-address",
      },
      {
        id: "system-email", label: "System email", description: "Quotations, invoices, notifications and password resets.",
        settings: [], block: "messaging.system-smtp", keywords: ["smtp", "transactional"],
        blockFields: [
          { label: "System from name", key: "email_smtp_from_name" }, { label: "System from email", key: "email_smtp_from_email" },
          { label: "System SMTP host", key: "email_smtp_host" }, { label: "System SMTP port", key: "email_smtp_port" },
          { label: "System SMTP username", key: "email_smtp_user" }, { label: "System SMTP password", key: "email_smtp_password" },
          { label: "System STARTTLS", key: "email_smtp_tls" },
        ],
      },
      {
        id: "crm-inboxes", label: "CRM inboxes",
        description: "The shared inboxes (hello@, sales@...), their outgoing and incoming servers, and the default one.",
        settings: [
          s("crm.email.relink_days", "Link threads of the last (days)", "Email threads of this many days are linked to a contact made after the emails arrived, and their emails logged on its history.", days),
          s("crm.email.relink_interval_minutes", "Link threads every (minutes)", "How often the inboxes look for threads to link to a new contact, lead or deal. Each sweep looks only at what changed since the last one.", minutes),
          s("crm.email.relink_full_hours", "Full link sweep every (hours)", "How often the sweep goes through every thread of the window instead of only what changed.", hours),
        ],
        block: "messaging.crm-inboxes", keywords: ["smtp", "imap", "inbox", "inboxes", "mailbox", "sales", "hello", "relink"],
        blockFields: [
          { label: "Add inbox", hint: "A new inbox is switched on after its servers are set and tested, then imports its recent mail quietly." },
          { label: "Default inbox" },
          { label: "CRM from name", key: "crm_smtp_from_name" }, { label: "CRM from email", key: "crm_smtp_from_email" },
          { label: "CRM SMTP host", key: "crm_smtp_host" }, { label: "CRM SMTP port", key: "crm_smtp_port" },
          { label: "CRM SMTP username", key: "crm_smtp_user" }, { label: "CRM SMTP password", key: "crm_smtp_password" },
          { label: "CRM STARTTLS", key: "crm_smtp_tls" },
          { label: "IMAP host", key: "crm_imap_host" }, { label: "IMAP port", key: "crm_imap_port" },
          { label: "IMAP username", key: "crm_imap_user" }, { label: "IMAP password", key: "crm_imap_password" },
          { label: "IMAP SSL/TLS", key: "crm_imap_ssl" },
        ],
      },
      {
        id: "mail-sync", label: "Mail sync", description: "Mailbox mirroring.",
        settings: [
          s("mail.sync_lock_ttl_seconds", "Lock per account folder", "How long one sync holds a folder.", seconds),
        ],
      },
      {
        id: "campaigns", label: "Email campaigns", description: "Sending pace of marketing email.",
        keywords: ["marketing"],
        settings: [
          s("marketing.email_daily_limit", "Most emails per day", "Company time."),
          s("marketing.email_min_delay_seconds", "Shortest gap between sends", undefined, seconds),
          s("marketing.email_max_delay_seconds", "Longest gap between sends", undefined, seconds),
          s("marketing.queue_lock_ttl_seconds", "Sender lease", "Must exceed the longest gap.", seconds),
        ],
      },
      {
        id: "whatsapp", label: "WhatsApp link", description: "The ERP's WhatsApp Web session.",
        settings: [
          s("wa_session_auto_reconnect", "Reconnect automatically", "When the WhatsApp session drops."),
          s("whatsapp.conversations_cache_minutes", "Keep the chat list for", "When WhatsApp is slow to answer, the inbox shows the last list it fetched (0: never).", { ...minutes, min: 0 }),
        ],
      },
    ],
  },
  {
    id: "approvals", label: "Approvals & signing", description: "Who approves what, and e-signatures.",
    sections: [
      {
        id: "policies", label: "Approval policies", description: "Who approves which documents, and when.",
        settings: [], block: "approvals.policies", keywords: ["approvals", "sop"],
      },
      {
        id: "signing", label: "E-signing", description: "Signature images and signer reminders.",
        settings: [
          s("sign.signature_max_bytes", "Largest signature image", "Signature or initials.", { unit: "bytes" }),
          s("sign.reminder_window_hours", "Remind signers when the deadline is within", undefined, hours),
          s("sign.reminder_interval_hours", "Between reminders to the same signer", undefined, hours),
          // 3 Nov 2026: outbound seal (migration 0152)
          s("documents.seal.outbound", "Digitally seal documents sent out",
            "Every approved quotation, invoice, order or proposal with a verification QR goes out with the company's digital seal, so a PDF reader flags any edit. Off: the unsealed copy is sent."),
        ],
        keywords: ["seal", "pades", "digital signature", "certified"],
      },
    ],
  },
  {
    id: "operations", label: "Operations", description: "Projects, procurement, manufacturing, fleet and maintenance.",
    sections: [
      {
        id: "projects", label: "Projects", description: "Labour cost of project hours.",
        settings: [
          s("projects.monthly_hours", "Working hours in a month", "Hourly cost is salary divided by this.", hours),
          s("projects.default_hourly_cost", "Hourly cost without an employee record", "In the functional currency."),
        ],
      },
      {
        id: "procurement", label: "Procurement", description: "Receipts and withholding tax.",
        settings: [
          s("procurement.receipt_over_tolerance_pct", "Over-delivery tolerance", "On a goods receipt.", pct),
          s("procurement.default_withholding_rate", "Withholding tax rate for new vendors", "Those that withhold it.", pct),
          s("vendor.portal.max_upload_mb", "Largest vendor document", "Per file a vendor uploads in the vendor portal (PDF, image, Word or Excel).", { unit: "MB", min: 1 }),
        ],
      },
      {
        id: "manufacturing", label: "Manufacturing", description: "Unit labels and generated serials for manufactured units.",
        keywords: ["label", "unit", "serial", "printer", "hardware"],
        settings: [
          s("manufacturing.label.width_mm", "Unit label width", "The label printer's label size.", { unit: "mm" }),
          s("manufacturing.label.height_mm", "Unit label height", undefined, { unit: "mm" }),
          s("manufacturing.serial.generate_tries", "Serial generation tries", "Draws before giving up when a generated unit serial is already taken (rare)."),
        ],
      },
      {
        id: "fleet", label: "Fleet & maintenance", description: "When due dates show as soon.",
        settings: [
          s("fleet.expiry_warning_days", "Fleet expiry warning", "Before insurance, registration or a service is due.", days),
          s("maintenance.due_soon_days", "Maintenance due soon", "Before a maintenance plan is due.", days),
        ],
      },
    ],
  },
  {
    id: "integrations", label: "Integrations & access", description: "MCP access, GitLab and users.",
    sections: [
      {
        id: "mcp", label: "MCP access", description: "Agents that work in the ERP for a user.",
        block: "integrations.mcp", keywords: ["agents", "tokens", "oauth"],
        settings: [
          s("mcp.oauth.access_minutes", "Login token lasts", "It renews itself.", minutes),
          s("mcp.oauth.refresh_days", "Stay logged in unused for", undefined, days),
          s("mcp.oauth.code_minutes", "Sign-in code valid for", undefined, minutes),
          s("mcp.pat.default_days", "Personal token lifetime", "Default for new tokens.", days),
          s("mcp.pat.max_days_power", "Longest life of a token that can act", "Tokens and connections that can act end after this many days at most.", days),
          s("mcp.owner.confirm_minutes", "Full-power confirm code valid for", "The Owner's code for messages, meetings and invoices.", minutes),
          s("mcp.apply.require_totp", "Applying needs the authenticator code",
            "With two-factor sign-in on, an agent applies a customer message, invoice, meeting, money change, delete or merge only with the code you type. Off: your Allow click alone applies it."),
        ],
      },
      {
        id: "users", label: "Users & roles", description: "Accounts, roles and permissions.",
        settings: [], block: "integrations.users", keywords: ["permissions", "roles"],
      },
      {
        id: "gitlab", label: "GitLab", description: "Projects and issues from GitLab.",
        settings: [
          // 17 Nov 2026: the issue sync's pace (the switch and the people are on the GitLab page)
          s("rnd.gitlab.sync_batch", "Issue sync: items per minute", "Backlog items and comments sent to each repository per minute; the rest go the next minute."),
          s("rnd.gitlab.pull_minutes", "Issue sync: pull every", "Recently updated issues are read from GitLab this often, so a missed webhook is caught.", minutes),
          s("rnd.gitlab.retry_max_minutes", "Issue sync: longest retry wait", "An item GitLab refused is tried again after 1, 2, 4 ... minutes, up to this.", minutes),
        ], block: "integrations.gitlab", keywords: ["git", "issues", "backlog", "sync"],
      },
    ],
  },
  {
    id: "system", label: "System & security", description: "Login security and everything else.",
    sections: [
      {
        id: "login-security", label: "Login security", description: "Limits on failed sign-ins and two-factor sign-in.",
        settings: [
          s("auth.login_max_attempts", "Failed logins allowed", "Per IP address per window."),
          s("auth.login_window_seconds", "Login limit window", undefined, seconds),
          s("auth.totp.issuer", "Two-factor: name in the app", "What authenticator apps show for these accounts (new setups only)."),
          s("auth.totp.challenge_seconds", "Two-factor: time to enter the code", "After a right password, at login.", seconds),
          s("auth.totp.max_failed_attempts", "Two-factor: wrong codes allowed", "Per person per window: login, approvals and Profile. Wrong codes at login also count toward the login limit."),
          s("auth.totp.failed_window_seconds", "Two-factor: wrong-code window", undefined, seconds),
          s("auth.totp.recovery_codes", "Two-factor: recovery codes", "One-time codes made at setup and on New recovery codes."),
        ],
      },
      {
        id: "video-calls", label: "Video calls", description: "Call size and the STUN servers browsers use.",
        keywords: ["call", "meeting", "webrtc", "stun", "turn", "participants"],
        settings: [
          s("calls.max_participants", "People per call", "The most people in one call, members and admitted guests.", { min: 2 }),
          s("calls.stun_urls", "Extra STUN servers",
            "A list of stun: URLs browsers may also use, such as [\"stun:stun.example.com:3478\"]. Empty: calls use only this installation's own TURN/STUN server, so no outside service sees who calls whom."),
        ],
      },
      {
        id: "public-pages", label: "Public pages", description: "Limits on what visitors without an account can send.",
        keywords: ["rate limit", "spam", "support", "registration", "waiting room"],
        settings: [
          s("public.forms.requests_per_minute", "Public form submissions per minute", "From one address: support tickets, vendor and partner sign-ups, joining a call's waiting room."),
          s("calls.guest_wait.max_waiting", "Guests waiting per call", "The most guests waiting to be admitted to one video call."),
        ],
      },
      {
        id: "other", label: "Other settings", description: "Settings the server has that no other section shows.",
        settings: [], block: "system.other",
      },
    ],
  },
];

/** Keys edited on another page or by a block's own API; never listed under Other. */
const MANAGED_ELSEWHERE_PREFIXES = ["branding.", "ai.", "currency.", "crm.followup.", "crm.enrich.", "email_smtp_", "crm_smtp_", "crm_imap_", "login_"];
const MANAGED_ELSEWHERE_KEYS = new Set([
  "documents.product_files_migrated",
  "nary.ack.templates", "nary.paused", "nary.shadow_mode", "nary.autonomy.levels", "nary.autonomy.never_promote",
  "nary.autonomy.never_promote_unlocked", "nary.google.client_id", "nary.google.client_secret",
]);

/** An extra CRM inbox's server keys (crm_sales_smtp_host...), edited in CRM inboxes. */
const EXTRA_INBOX_KEY = /^crm_[a-z0-9_]+_(smtp|imap)_[a-z_]+$/;

export function isManagedElsewhere(key: string) {
  return MANAGED_ELSEWHERE_KEYS.has(key) || MANAGED_ELSEWHERE_PREFIXES.some((p) => key.startsWith(p)) || EXTRA_INBOX_KEY.test(key);
}

/** Every key a section lists as a setting. */
export function registryKeys(): string[] {
  return SETTINGS_AREAS.flatMap((a) => a.sections.flatMap((s) => s.settings.map((d) => d.key)));
}

const KNOWN = new Set(registryKeys());

/** Keys from the server that belong under System & security, Other. */
export function otherKeys(serverKeys: string[]): string[] {
  return serverKeys.filter((k) => !KNOWN.has(k) && !isManagedElsewhere(k)).sort();
}

export function findArea(id: string | null | undefined): SettingsArea | undefined {
  return SETTINGS_AREAS.find((a) => a.id === id);
}

/** Where a setting key lives. */
export function locateSetting(key: string): { area: SettingsArea; section: SettingsSection; def?: SettingDef } | undefined {
  for (const area of SETTINGS_AREAS) {
    for (const section of area.sections) {
      const def = section.settings.find((d) => d.key === key);
      if (def) return { area, section, def };
      if (section.blockFields?.some((f) => f.key === key)) return { area, section };
    }
  }
  return undefined;
}

/** Where a setting lives on the Settings page. */
export function settingsHref(areaId: string, sectionId?: string) {
  return `/settings?area=${encodeURIComponent(areaId)}${sectionId ? `#${encodeURIComponent(sectionId)}` : ""}`;
}

/** The old Settings tabs (?tab=...) and anchors, mapped to their new place. */
export const LEGACY_TABS: Record<string, { area: string; section?: string }> = {
  general: { area: "company" },
  email: { area: "messaging" },
  branding: { area: "company", section: "logo" },
  hr: { area: "people" },
  ai: { area: "nary" },
  ai_knowledge: { area: "nary", section: "knowledge" },
  crm_attributes: { area: "sales", section: "crm-attributes" },
  followups: { area: "sales", section: "follow-ups" },
  mcp: { area: "integrations", section: "mcp" },
  approvals: { area: "approvals", section: "policies" },
};

/**
 * The area and section a /settings URL means: ?area=&section= or #section, an old ?tab=, an old
 * #anchor (#payroll), or a setting key (?key=). Unknown or missing means the first area.
 */
export function resolveSettingsLocation(params: { area?: string | null; tab?: string | null; section?: string | null; key?: string | null; hash?: string | null }): { area: string; section?: string } {
  const hash = params.hash ? decodeURIComponent(params.hash.replace(/^#/, "")) : "";
  if (params.key) {
    const hit = locateSetting(params.key);
    if (hit) return { area: hit.area.id, section: hit.section.id };
  }
  const sectionIn = (a: SettingsArea, id?: string | null) => (id && a.sections.some((s) => s.id === id) ? id : undefined);
  const area = findArea(params.area);
  if (area) return { area: area.id, section: sectionIn(area, params.section) ?? sectionIn(area, hash) };
  const legacy = params.tab ? LEGACY_TABS[params.tab] : undefined;
  if (legacy) return { area: legacy.area, section: legacy.section ?? (sectionIn(findArea(legacy.area)!, hash)) };
  if (hash) {
    for (const a of SETTINGS_AREAS) if (sectionIn(a, hash)) return { area: a.id, section: hash };
  }
  return { area: SETTINGS_AREAS[0].id };
}

/** True when a stored value differs from the known default (numbers and JSON compared by value). */
export function differsFromDefault(def: SettingDef, value: string | null | undefined): boolean {
  if (def.default === undefined || value === undefined || value === null) return false;
  const a = value.trim();
  const b = def.default.trim();
  if (a === b) return false;
  if (def.type === "number" || def.type === "percent" || def.type === "money") {
    const x = Number(a), y = Number(b);
    if (a !== "" && b !== "" && Number.isFinite(x) && Number.isFinite(y)) return x !== y;
  }
  if (def.type === "boolean") return a.toLowerCase() !== b.toLowerCase();
  return true;
}

export interface SettingsSearchEntry {
  id: string;
  label: string;
  /** "Money & tax / Invoices" */
  trail: string;
  hint?: string;
  /** Raw key, matched too. */
  key?: string;
  href: string;
}

/** Everything searchable on the Settings page: areas, sections and each setting. */
export function settingsSearchEntries(): SettingsSearchEntry[] {
  const out: SettingsSearchEntry[] = [];
  for (const a of SETTINGS_AREAS) {
    out.push({ id: `area:${a.id}`, label: a.label, trail: "Settings", hint: a.description, href: settingsHref(a.id) });
    for (const s of a.sections) {
      out.push({ id: `section:${a.id}:${s.id}`, label: s.label, trail: `Settings / ${a.label}`, hint: s.description, href: settingsHref(a.id, s.id) });
      for (const d of s.settings) {
        out.push({ id: `setting:${d.key}`, label: d.label, trail: `${a.label} / ${s.label}`, hint: d.hint, key: d.key, href: settingsHref(a.id, s.id) });
      }
      s.blockFields?.forEach((f, i) => {
        out.push({ id: `field:${a.id}:${s.id}:${f.key ?? i}`, label: f.label, trail: `${a.label} / ${s.label}`, hint: f.hint, key: f.key, href: settingsHref(a.id, s.id) });
      });
    }
  }
  return out;
}

export interface SettingsSearchHit extends SettingsSearchEntry {
  areaId: string;
  sectionId?: string;
  kind: "area" | "section" | "setting";
}

const norm = (t: string) => t.toLowerCase().normalize("NFKD").replace(/[̀-ͯ]/g, "");

/**
 * Search the settings by label, hint, key, section and area. Every word must match somewhere;
 * a label match ranks above a hint or key match. Areas and sections come before settings.
 */
export function searchSettings(query: string, limit = 50): SettingsSearchHit[] {
  const words = norm(query).split(/\s+/).filter(Boolean);
  if (!words.length) return [];
  const hits: { hit: SettingsSearchHit; score: number }[] = [];
  const consider = (hit: SettingsSearchHit, label: string, rest: string[], base: number) => {
    const l = norm(label);
    const hay = norm([label, ...rest].join(" "));
    if (!words.every((w) => hay.includes(w))) return;
    const inLabel = words.filter((w) => l.includes(w)).length;
    hits.push({ hit, score: base + inLabel * 10 + (l.startsWith(words[0]) ? 5 : 0) });
  };
  for (const a of SETTINGS_AREAS) {
    consider({ id: `area:${a.id}`, label: a.label, trail: "Settings", hint: a.description, href: settingsHref(a.id), areaId: a.id, kind: "area" },
      a.label, [a.description], 2);
    for (const s of a.sections) {
      consider({ id: `section:${a.id}:${s.id}`, label: s.label, trail: a.label, hint: s.description, href: settingsHref(a.id, s.id), areaId: a.id, sectionId: s.id, kind: "section" },
        s.label, [s.description ?? "", a.label, ...(s.keywords ?? [])], 1);
      for (const d of s.settings) {
        consider({ id: `setting:${d.key}`, label: d.label, trail: `${a.label} / ${s.label}`, hint: d.hint, key: d.key, href: settingsHref(a.id, s.id), areaId: a.id, sectionId: s.id, kind: "setting" },
          d.label, [d.hint ?? "", d.key, s.label, a.label, ...(s.keywords ?? [])], 0);
      }
      s.blockFields?.forEach((f, i) => {
        consider({ id: `field:${a.id}:${s.id}:${f.key ?? i}`, label: f.label, trail: `${a.label} / ${s.label}`, hint: f.hint, key: f.key, href: settingsHref(a.id, s.id), areaId: a.id, sectionId: s.id, kind: "setting" },
          f.label, [f.hint ?? "", f.key ?? "", s.label, a.label, ...(s.keywords ?? [])], 0);
      });
    }
  }
  return hits.sort((x, y) => y.score - x.score).slice(0, limit).map((h) => h.hit);
}
