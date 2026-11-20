import { describe, it, expect } from "vitest";
import {
  LEGACY_TABS, SETTINGS_AREAS, SETTINGS_BLOCKS, differsFromDefault, locateSetting, otherKeys, registryKeys,
  resolveSettingsLocation, searchSettings, settingsSearchEntries, type SettingDef,
} from "./settings-registry";

/** Every key the old Settings page edited (its groups, HR and payroll forms). */
const LEGACY_KEYS = [
  "ai.assistant.catalog_digest_max_items", "ai.erp_guide.enabled", "auth.login_max_attempts",
  "auth.login_window_seconds", "catalog.conversion_base_currency", "catalog.independent_currencies", "company_address",
  "company_email", "company_name", "company_registration_no", "company_tax_id", "company_phone", "company_rd_office",
  "company_signatory", "company_signatory_title", "company_trade_name", "company_website", "crm.activity_types",
  "crm.funnel.default_lead_owner_email", "crm.funnel.disqualify_reasons", "crm.funnel.lead_priorities",
  "crm.funnel.lead_sources", "crm.funnel.loss_reasons", "crm.funnel.stage_probabilities", "crm.scheduled.max_attempts",
  "crm.scheduled.stuck_after_minutes", "currency.decimals", "date_format", "default_currency",
  "documents.proposal.classification", "documents.proposal.labels", "documents.proposal.prepared_by",
  "documents.proposal.template", "documents.proposal.validity_days", "finance.default_sales_tax",
  "finance.gl_autopost_from", "finance.invoice_reminder_days", "finance.invoice_reminder_repeat_days",
  "fleet.expiry_warning_days", "hr.face_match_distance", "hr.geo_max_accuracy_m", "hr.overnight_shift_hours",
  "hr.work_start_time", "invoice_prefix", "mail.sync_lock_ttl_seconds", "maintenance.due_soon_days",
  "marketing.email_daily_limit", "marketing.email_max_delay_seconds", "marketing.email_min_delay_seconds",
  "marketing.queue_lock_ttl_seconds", "mcp.owner.confirm_minutes", "mcp.oauth.access_minutes", "mcp.oauth.code_minutes",
  "mcp.oauth.refresh_days", "mcp.pat.default_days", "mcp.pat.max_days_power", "nary.ack.default_language", "nary.ack.enabled",
  "nary.ack.late_after_minutes", "nary.ack.ongoing_days", "nary.autonomy.demotion_floor",
  "nary.autonomy.edit_threshold_pct", "nary.autonomy.promotion_count", "nary.brief_times",
  "nary.briefs.items_per_section", "nary.briefs.snooze_days", "nary.cadence.at_risk_silent_days",
  "nary.cadence.max_without_news", "nary.cadence.min_gap_days", "nary.cadence.nurture_silent_days",
  "nary.cadence.recontact_days", "nary.cadence.steps", "nary.owner.alert_stale_hours", "nary.owner.bundle_minutes",
  "nary.owner.confirm_twice_actions", "nary.owner.consult_proposals", "nary.owner.consult_quotations",
  "nary.owner.default_language", "nary.owner.enabled", "nary.owner.one_at_a_time", "nary.owner.quiet_minutes",
  "nary.owner.remind_working_hours", "nary.owner.topics_in_context", "nary.owner.urgent_kinds", "nary.contract.nudge_days",
  "nary.contract.won_needs_dp", "nary.email.intake_enabled", "nary.email.reply_window_hours",
  "nary.email.signature_fields", "nary.email.skip_rules", "nary.errors.dedupe_hours", "nary.events.batch_size",
  "nary.events.max_attempts", "nary.google.account_email", "nary.google.calendar_id", "nary.handoff.kickoff_tasks",
  "nary.heartbeat_minutes", "nary.history.batch_size", "nary.history.chats_per_run", "nary.history.full_read_months",
  "nary.history.missed_after_days", "nary.history.page_size", "nary.history.pages_per_run",
  "nary.history.pause_seconds", "nary.history.run_minutes", "nary.history.style_sample", "nary.history.sync_attempts",
  "nary.identity.default_country", "nary.identity.honorifics", "nary.identity.prefill_labels",
  "nary.identity.recent_form_days", "nary.identity.signal_scores", "nary.identity.thresholds",
  "nary.intake.arrival_window_minutes", "nary.intake.currency_by_country", "nary.meetings.customer_hours",
  "nary.meetings.holidays", "nary.meetings.reduced_hours_periods", "nary.meetings.reminders", "nary.meetings.rules",
  "nary.meetings.text", "nary.negotiation.competitor_checklist", "nary.negotiation.scripts", "nary.partners.routing",
  "nary.partners.warn_days", "nary.pending.expire_days", "nary.price.catalog_terms", "nary.reply.backfill_window_hours",
  "nary.reply.debounce_seconds", "nary.reply.max_lines", "nary.reply.max_questions", "nary.reply.questions",
  "nary.reply.unanswered_business_hours", "nary.reply.unanswered_lookback_days", "nary.reply_alerts.enabled",
  "nary.reply_alerts.mode", "nary.report.kpi_targets", "nary.report.response_sla_minutes", "nary.report.weekday",
  "nary.report.window_days", "nary.score.boundary_margin", "nary.score.classes", "nary.score.green_flags",
  "nary.score.red_flags", "nary.score.rubric", "nary.shadow_exempt_actions", "nary.skills.max_in_prompt",
  "nary.skills.tool_ideas_project", "nary.sop_revisions.briefs", "nary.staff_chat.bubble_chars",
  "nary.staff_chat.card_window_minutes", "nary.staff_chat.enabled", "nary.staff_chat.late_reply_hours",
  "nary.staff_chat.max_bubbles", "nary.staff_chat.max_lines", "nary.staff_chat.session_hours", "nary.staff_chat.voice",
  "nary.staff_chat.wait_seconds", "nary.stt.max_seconds", "nary.style.refresh_days", "nary.triage.business_signals",
  "nary.triage.example_count", "nary.urgent", "nary.wa.burst_alert_messages", "nary.wa.burst_alert_minutes",
  "nary.wa.card_notices", "nary.wa.chat_max_messages", "nary.wa.chat_window_minutes", "nary.wa.complaint_pause_hours",
  "nary.wa.gapcheck_grace_seconds", "nary.wa.gapcheck_lookback_hours", "nary.wa.gapcheck_minutes",
  "nary.wa.greeting_hours", "nary.wa.health_backlog_minutes", "nary.wa.health_minutes", "nary.wa.label_map",
  "nary.wa.labels_enabled", "nary.wa.max_per_day", "nary.wa.max_per_hour", "nary.wa.media_max_mb", "nary.wa.signature",
  "nary.wa.takeover_hours", "nary.web3forms.dkim_domains", "nary.web3forms.interest_map", "nary.web3forms.known_fields",
  "nary.web3forms.poll_junk", "nary.web3forms.required_fields", "nary.web3forms.routes",
  "nary.web3forms.sender_domains", "nary.working_hours", "nary.writing.lint_enabled", "payment_terms_default",
  "payroll.contributions", "payroll.income_tax", "payroll.overtime_hours_divisor",
  "payroll.overtime_multipliers", "payroll.rest_weekdays",
  "procurement.default_withholding_rate", "procurement.receipt_over_tolerance_pct",
  "proforma_prefix", "projects.default_hourly_cost",
  "projects.monthly_hours", "proposal.cost.contingency_pct", "proposal.cost.days_per_month",
  "proposal.cost.load_factor", "proposal.cost.min_monthly", "proposal.cost.update_threshold_pct",
  "proposal.discovery.fields", "proposal.discovery.min_coverage_pct", "quotation_prefix", "quotation_validity_days", "sales.margins",
  "sales.quotation.expire_grace_days", "sales.quotation.expired_nudge_days", "sales.service_down_payment_pct",
  "sales.split_payment_min_unit_equivalents", "sales.split_payment_on_order_pct", "sign.reminder_interval_hours",
  "sign.reminder_window_hours", "sign.signature_max_bytes", "timezone"
];

/** Keys the old Email tab and the branding forms edited through their own APIs. */
const BLOCK_KEYS = [
  "email_smtp_host", "email_smtp_password", "crm_smtp_host", "crm_imap_host", "crm_imap_ssl",
  "branding.logo_type", "branding.favicon_data", "currency.functional", "currency.active", "currency.api_key",
  "nary.ack.templates", "crm.followup.no_reply_days", "crm.enrich.min_confidence", "ai.routing", "ai.budget.monthly_usd",
];

describe("settings registry", () => {
  it("places every setting of the old page in exactly one section", () => {
    const keys = registryKeys();
    for (const k of LEGACY_KEYS) {
      expect(keys.filter((x) => x === k), k).toHaveLength(1);
    }
    expect(new Set(keys).size).toBe(keys.length);
  });

  it("finds a home for the keys the blocks edit", () => {
    for (const k of BLOCK_KEYS) expect(locateSetting(k), k).toBeDefined();
  });

  it("renders every custom block exactly once and has ten areas", () => {
    expect(SETTINGS_AREAS.map((a) => a.label)).toEqual([
      "Company & branding", "Sales & CRM", "Money & tax", "People & payroll", "Nary & AI", "Email & WhatsApp",
      "Approvals & signing", "Operations", "Integrations & access", "System & security",
    ]);
    const used = SETTINGS_AREAS.flatMap((a) => a.sections.map((s) => s.block).filter(Boolean));
    expect([...used].sort()).toEqual([...SETTINGS_BLOCKS].sort());
  });

  it("keeps section ids unique so an old #anchor has one place", () => {
    const ids = SETTINGS_AREAS.flatMap((a) => a.sections.map((s) => s.id));
    expect(new Set(ids).size).toBe(ids.length);
  });

  it("gives every setting a label, a type and no em dash in its copy", () => {
    for (const a of SETTINGS_AREAS) for (const s of a.sections) {
      for (const text of [a.label, a.description, s.label, s.description ?? ""]) expect(text).not.toMatch(/—/);
      for (const d of s.settings) {
        expect(d.label, d.key).toBeTruthy();
        expect(d.type, d.key).toBeTruthy();
        expect(`${d.label} ${d.hint ?? ""}`, d.key).not.toMatch(/—/);
      }
    }
  });

  it("sends the old tabs and anchors to their area and section", () => {
    expect(resolveSettingsLocation({ tab: "hr", hash: "#payroll" })).toEqual({ area: "people", section: "payroll" });
    expect(resolveSettingsLocation({ tab: "ai_knowledge" })).toEqual({ area: "nary", section: "knowledge" });
    expect(resolveSettingsLocation({ tab: "general" })).toEqual({ area: "company", section: undefined });
    expect(resolveSettingsLocation({ hash: "#payroll" })).toEqual({ area: "people", section: "payroll" });
    expect(resolveSettingsLocation({ area: "money", hash: "#invoices" })).toEqual({ area: "money", section: "invoices" });
    expect(resolveSettingsLocation({ key: "nary.wa.signature" })).toEqual({ area: "nary", section: "whatsapp-channel" });
    expect(resolveSettingsLocation({ area: "nope" })).toEqual({ area: "company" });
    for (const t of Object.keys(LEGACY_TABS)) expect(resolveSettingsLocation({ tab: t }).area).toBe(LEGACY_TABS[t].area);
  });

  it("places the two-factor sign-in settings under Login security with their defaults", () => {
    for (const [k, d] of [["auth.totp.issuer", "Granary"], ["auth.totp.challenge_seconds", "300"],
      ["auth.totp.max_failed_attempts", "5"], ["auth.totp.failed_window_seconds", "900"], ["auth.totp.recovery_codes", "10"]]) {
      const at = locateSetting(k);
      expect(at?.section.id, k).toBe("login-security");
      expect(at?.def?.default, k).toBe(d);
    }
  });

  it("lists server settings nobody else shows under Other", () => {
    expect(otherKeys(["company_name", "nary.paused", "branding.logo_text", "some.new_key"])).toEqual(["some.new_key"]);
  });

  it("marks a value that differs from its default", () => {
    const n: SettingDef = { key: "x", label: "X", type: "number", default: "72" };
    expect(differsFromDefault(n, "72.0")).toBe(false);
    expect(differsFromDefault(n, "48")).toBe(true);
    expect(differsFromDefault({ ...n, default: undefined }, "48")).toBe(false);
  });
});

describe("settings search", () => {
  it("matches label, hint, key and section, every word", () => {
    expect(searchSettings("whatsapp sign-off").map((h) => h.key)).toContain("nary.wa.signature");
    expect(searchSettings("payroll.income").map((h) => h.key)).toContain("payroll.income_tax");
    expect(searchSettings("smtp host").map((h) => h.key)).toEqual(expect.arrayContaining(["email_smtp_host", "crm_smtp_host"]));
    expect(searchSettings("contributions employer").map((h) => h.key)).toContain("payroll.contributions");
    expect(searchSettings("zzz nothing")).toEqual([]);
    expect(searchSettings("  ")).toEqual([]);
  });

  it("ranks a label match first and links to the section", () => {
    const [first] = searchSettings("complaint");
    expect(first.key).toBe("nary.wa.complaint_pause_hours");
    expect(first.href).toBe("/settings?area=nary#replies");
  });

  it("keeps the palette entries: areas, sections and settings", () => {
    const entries = settingsSearchEntries();
    expect(entries.find((e) => e.id === "area:people")?.href).toBe("/settings?area=people");
    expect(entries.find((e) => e.key === "payroll.income_tax")).toMatchObject({ trail: "People & payroll / Payroll", href: "/settings?area=people#payroll" });
  });

  // visuals V4
  it("places the proposal figure settings under Sales & CRM, Proposals with their defaults", () => {
    const want: Record<string, [string, string]> = {
      "proposal.visuals.max_attempts": ["number", "3"], "proposal.visuals.vision_check": ["boolean", "true"],
      "proposal.visuals.default_figures": ["json", '["timeline","architecture"]'], "proposal.visuals.custom_svg_max_kb": ["number", "300"],
    };
    for (const [key, [type, def]] of Object.entries(want)) {
      const hit = locateSetting(key)!;
      expect([hit.area.id, hit.section.id], key).toEqual(["sales", "proposals"]);
      expect(hit.def, key).toMatchObject({ type, default: def });
    }
    expect(searchSettings("diagram").some((r) => JSON.stringify(r).includes("proposal.visuals"))).toBe(true);
  });

  // fix-longapply
  it("places the long-work settings under Nary, Autonomy & approvals with their defaults", () => {
    const want: Record<string, string> = { "nary.apply_wait_seconds": "45", "nary.applying_stale_minutes": "30" };
    for (const [key, def] of Object.entries(want)) {
      const hit = locateSetting(key)!;
      expect([hit.area.id, hit.section.id], key).toEqual(["nary", "autonomy"]);
      expect(hit.def, key).toMatchObject({ type: "number", default: def });
    }
  });

  // multi-inbox
  it("places the automatic-message gap, duplicate deal window and meeting offer patterns", () => {
    const want: Record<string, [string, string, string | undefined]> = {
      "nary.autosend.min_gap_hours": ["follow-up-cadence", "number", "12"],
      "crm.deal_dup.window_days": ["funnel", "number", "30"],
      "nary.writing.meeting_offer_patterns": ["replies", "json", undefined],
    };
    for (const [key, [section, type, def]] of Object.entries(want)) {
      const hit = locateSetting(key)!;
      expect(hit.section.id, key).toBe(section);
      expect(hit.def, key).toMatchObject({ type, default: def });
    }
  });

  // security 27 Oct (migration 0138)
  it("places the email relink sweep and the duplicate ask-again settings with their defaults", () => {
    const want: Record<string, [string, string]> = {
      "crm.email.relink_days": ["crm-inboxes", "180"],
      "crm.email.relink_interval_minutes": ["crm-inboxes", "15"],
      "crm.email.relink_full_hours": ["crm-inboxes", "24"],
      "crm.dup.repropose_days": ["funnel", "30"],
    };
    for (const [key, [section, def]] of Object.entries(want)) {
      const hit = locateSetting(key)!;
      expect(hit.section.id, key).toBe(section);
      expect(hit.def, key).toMatchObject({ type: "number", default: def });
    }
  });

  it("finds the CRM inboxes section and keeps extra inbox server keys out of Other", () => {
    for (const q of ["smtp", "imap", "inbox", "sales inbox"]) {
      expect(searchSettings(q).some((h) => h.sectionId === "crm-inboxes"), q).toBe(true);
    }
    expect(otherKeys(["crm_sales_smtp_host", "crm_sales_imap_password", "crm.deal_dup.window_days", "x.y"])).toEqual(["x.y"]);
  });
});

describe("settings of later releases", () => {
  it("places the work queue rows under Nary, Work queue, with their defaults (3 Nov 2026)", () => {
    const want: Record<string, [string, string]> = {
      "nary.queue.nudge_enabled": ["boolean", "true"],
      "nary.queue.nudge_times": ["json", "[\"08:00\", \"13:00\", \"17:00\"]"],
      "nary.queue.urgent_gap_minutes": ["number", "30"], "nary.queue.nudge_items": ["number", "5"],
      "nary.queue.quotation_expiry_days": ["number", "7"], "nary.queue.meeting_reminder_hours": ["number", "36"],
      "nary.queue.meeting_brief_hours": ["number", "3"], "nary.queue.card_expiry_hours": ["number", "24"],
      "nary.queue.new_lead_days": ["number", "14"],
      "nary.queue.default_limit": ["number", "50"],
    };
    for (const [key, [type, def]] of Object.entries(want)) {
      const hit = locateSetting(key)!;
      expect(hit, key).toBeTruthy();
      expect(hit.section.id, key).toBe("work-queue");
      expect(hit.def?.type, key).toBe(type);
      expect(hit.def?.default, key).toBe(def);
    }
    for (const key of ["nary.queue.urgent_kinds", "nary.queue.today_kinds"]) expect(locateSetting(key)?.section.id, key).toBe("work-queue");
    expect(searchSettings("nudge").some((r) => JSON.stringify(r).includes("nary.queue"))).toBe(true);
  });

  it("puts the quotation notes templates in their own Sales section, with their numbers (4 Nov 2026)", () => {
    const want: Record<string, [string, string | undefined]> = {
      "sales.quotation.notes.block_submit": ["boolean", "true"],
      "sales.quotation.notes.lead_days_min": ["number", "10"],
      "sales.quotation.notes.lead_days_max": ["number", "15"],
      "sales.quotation.notes.invoice_due_days": ["number", "14"],
      "sales.quotation.notes.non_payment_pause_days": ["number", "30"],
      "sales.quotation.notes.non_payment_terminate_days": ["number", "60"],
      "sales.quotation.notes.warranty_years": ["number", "1"],
      "sales.quotation.notes_template.products": ["json", undefined],
      "sales.quotation.notes.texts": ["json", undefined],
    };
    for (const [key, [type, def]] of Object.entries(want)) {
      const hit = locateSetting(key)!;
      expect(hit, key).toBeTruthy();
      expect(hit.section.id, key).toBe("quotation-notes");
      expect(hit.def?.type, key).toBe(type);
      expect(hit.def?.default, key).toBe(def);
      expect(`${hit.def?.label} ${hit.def?.hint}`).not.toMatch(/—/);
    }
    expect(searchSettings("notes template").some((r) => JSON.stringify(r).includes("quotation.notes"))).toBe(true);
  });
  it("places the segments with the CRM picklists, empty by default (free text)", () => {
    const hit = locateSetting("crm.segments")!;
    expect(hit).toBeTruthy();
    expect(hit.def?.type).toBe("json");
    expect(hit.def?.default).toBe("[]");
    expect(hit.section.settings.some((r) => r.key === "crm.funnel.lead_sources")).toBe(true);
    expect(searchSettings("segment").some((r) => JSON.stringify(r).includes("crm.segments"))).toBe(true);
  });

  it("ships none of the removed modules' settings", () => {
    for (const prefix of ["website.", "licensing.", "integration.mail.", "offering.", "crm.offering.", "nary.claims."]) {
      expect(registryKeys().filter((k) => k.startsWith(prefix)), prefix).toEqual([]);
    }
    expect(SETTINGS_BLOCKS).not.toContain("integrations.licensing");
    expect(SETTINGS_BLOCKS).not.toContain("integrations.website");
  });
});

// batch E: call size and STUN list are settings
describe("video call settings", () => {
  it("live under System & security, Video calls with their defaults", () => {
    const want: Record<string, [string, string]> = {
      "calls.max_participants": ["number", "10"], "calls.stun_urls": ["json", "[]"],
    };
    for (const [key, [type, def]] of Object.entries(want)) {
      const hit = locateSetting(key)!;
      expect([hit.area.id, hit.section.id], key).toEqual(["system", "video-calls"]);
      expect(hit.def, key).toMatchObject({ type, default: def });
    }
    expect(searchSettings("stun").map((h) => h.key)).toContain("calls.stun_urls");
  });
});
