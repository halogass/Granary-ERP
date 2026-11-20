"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import Link from "next/link";
import { useRouter, useSearchParams } from "next/navigation";
import { ArrowRight } from "lucide-react";
import { systemApi } from "@/lib/api";
import { invalidateCompanyCache } from "@/lib/print";
import { scrollPaneTo } from "@/lib/scrollPane";
import { Button, Card, CardBody, EmptyState, PageHeader, Skeleton } from "@/components/ui";
import {
  findArea, otherKeys, resolveSettingsLocation, settingsHref,
  type SettingDef, type SettingsArea, type SettingsBlockId, type SettingsSection, type SettingType,
} from "@/lib/settings-registry";
import { SaveBar, SettingsDraftProvider, useDraftSource, useDraftSummary, useLeaveGuard } from "./SettingsDraft";
import { SettingRow, isEdited, validateSetting } from "./SettingRow";
import { OnThisPage, SettingsNav } from "./SettingsNav";
import { BrandingProvider, FaviconBlock, LetterheadBlock, LoginTextBlock, LogoBlock } from "./BrandingBlocks";
import { EmailConfigProvider, SystemSmtpBlock, TestAddressBlock } from "./EmailServerBlocks";
import CrmInboxesBlock from "./CrmInboxesBlock";
import CurrencyBlock from "./CurrencyBlock";
import OfficeLocationsBlock, { KiosksBlock } from "./OfficeLocationsBlock";
import {
  AiBudgetSection, AiEnrichmentBlock, AiGeneralBlock, AiKeysBlock, AiKnowledgeBlock, AiPersonaBlock, AiResearchBlock,
  AiRoutingSection, AiScoringBlock, AiSettingsProvider,
} from "./AiSettingsBlocks";
import CrmAttributesTab from "./CrmAttributesTab";
import FollowUpsSettingsTab from "./FollowUpsSettingsTab";
import RateCardBlock from "./RateCardBlock";
import DeletedActivitiesBlock from "./DeletedActivitiesBlock";
import NaryAckTemplatesBlock from "./NaryAckTemplatesBlock";
import EmailSignatureTemplate from "./EmailSignatureTemplate";
import ApprovalsSettingsTab from "./ApprovalsSettingsTab";
import McpTokensTab from "./McpTokensTab";
import DocumentCategoriesBlock from "./DocumentCategoriesBlock";

interface ServerSetting { key: string; value: string | null; value_type?: string | null; label?: string | null }

interface BlockProps {
  server: Record<string, ServerSetting>;
  reload: () => Promise<void>;
  other: React.ReactNode;
}

function LinkCard({ title, text, href, action }: { title: string; text: string; href: string; action: string }) {
  return (
    <Card>
      <CardBody className="flex flex-wrap items-center gap-3">
        <div className="min-w-0 flex-1">
          <p className="text-sm font-medium text-fg">{title}</p>
          <p className="text-xs text-fg-muted">{text}</p>
        </div>
        <Button asChild><Link href={href}>{action} <ArrowRight aria-hidden /></Link></Button>
      </CardBody>
    </Card>
  );
}

/** Every custom block of the registry; the type makes a missing one a compile error. */
export const BLOCKS: Record<SettingsBlockId, (p: BlockProps) => React.ReactNode> = {
  "company.logo": () => <LogoBlock />,
  "company.favicon": () => <FaviconBlock />,
  "company.login-text": () => <LoginTextBlock />,
  "company.letterhead": () => <LetterheadBlock />,
  "company.document-categories": () => <DocumentCategoriesBlock />,
  "sales.crm-attributes": () => <CrmAttributesTab />,
  "sales.followups": () => <FollowUpsSettingsTab />,
  "sales.rate-card": () => <RateCardBlock />,
  "sales.deleted-activities": () => <DeletedActivitiesBlock />,
  "money.currency": () => <CurrencyBlock />,
  "money.taxes": () => (
    <LinkCard title="Tax configurations and rules" text="Sales taxes, withholding and the active tax configuration are managed in Finance (one source)."
      href="/finance/taxes" action="Open taxes" />
  ),
  "people.offices": () => <OfficeLocationsBlock />,
  "people.kiosks": () => <KiosksBlock />,
  "ai.general": () => <AiGeneralBlock />,
  "ai.keys": () => <AiKeysBlock />,
  "ai.routing": () => <AiRoutingSection />,
  "ai.budget": () => <AiBudgetSection />,
  "ai.persona": () => <AiPersonaBlock />,
  "ai.research": () => <AiResearchBlock />,
  "ai.enrichment": () => <AiEnrichmentBlock />,
  "ai.scoring": () => <AiScoringBlock />,
  "ai.knowledge": () => <AiKnowledgeBlock />,
  "nary.ack-templates": (p) => (
    <NaryAckTemplatesBlock value={p.server["nary.ack.templates"]?.value ?? undefined} onSaved={() => { void p.reload(); }} />
  ),
  "messaging.signature-template": () => <EmailSignatureTemplate />,
  "messaging.test-address": () => <TestAddressBlock />,
  "messaging.system-smtp": () => <SystemSmtpBlock />,
  "messaging.crm-inboxes": () => <CrmInboxesBlock />,
  "approvals.policies": () => <ApprovalsSettingsTab />,
  "integrations.mcp": () => <McpTokensTab />,
  "integrations.users": () => (
    <LinkCard title="Users" text="Accounts, roles, portal access and passwords." href="/settings/users" action="Manage users" />
  ),
  "integrations.gitlab": () => (
    <LinkCard title="GitLab integration" text="GitLab servers and the repositories linked to projects." href="/settings/gitlab" action="Open GitLab" />
  ),
  "system.other": (p) => p.other,
};

/** Area-wide data providers for the blocks that share one form (branding, AI, mail servers). */
function AreaProviders({ areaId, children }: { areaId: string; children: React.ReactNode }) {
  if (areaId === "company") return <BrandingProvider>{children}</BrandingProvider>;
  if (areaId === "nary") return <AiSettingsProvider>{children}</AiSettingsProvider>;
  if (areaId === "messaging") return <EmailConfigProvider>{children}</EmailConfigProvider>;
  return <>{children}</>;
}

function typeOfServer(vt: string | null | undefined, value: string | null): SettingType {
  const t = (vt ?? "").toLowerCase();
  if (t === "json") return "json";
  if (t === "bool" || t === "boolean" || value === "true" || value === "false") return "boolean";
  if (["int", "integer", "decimal", "float", "number"].includes(t)) return "number";
  return "text";
}

/** A setting the server has but no section lists: shown under System & security, Other. */
function otherDef(row: ServerSetting): SettingDef {
  return { key: row.key, label: row.label || row.key, type: typeOfServer(row.value_type, row.value) };
}

export default function SettingsView() {
  return (
    <SettingsDraftProvider>
      <SettingsBody />
    </SettingsDraftProvider>
  );
}

function SettingsBody() {
  const router = useRouter();
  const params = useSearchParams();
  // null until read on the client: the redirect below waits for it (#payroll and the like).
  const [hash, setHash] = useState<string | null>(null);
  useEffect(() => {
    const read = () => setHash(window.location.hash);
    read();
    window.addEventListener("hashchange", read);
    return () => window.removeEventListener("hashchange", read);
  }, []);

  const loc = resolveSettingsLocation({
    area: params.get("area"), tab: params.get("tab"), section: params.get("section"), key: params.get("key"), hash: hash ?? "",
  });
  const area = findArea(loc.area)!;
  const focus = params.get("focus") ?? params.get("key");

  // Old URLs (?tab=..., #payroll, ?key=, no area) land on the canonical address.
  useEffect(() => {
    if (hash === null) return;
    if (params.get("area") === loc.area && !params.get("tab") && !params.get("key")) return;
    const href = settingsHref(loc.area, loc.section);
    router.replace(focus ? href.replace("?", `?focus=${encodeURIComponent(focus)}&`) : href);
  }, [hash, params, loc.area, loc.section, focus, router]);

  const [server, setServer] = useState<Record<string, ServerSetting> | null>(null);
  const [loadError, setLoadError] = useState(false);
  const reload = useCallback(async () => {
    try {
      const res = await systemApi.listSettings();
      const rows: ServerSetting[] = res.data.items ?? res.data ?? [];
      setServer(Object.fromEntries(rows.map((r) => [r.key, r])));
      setLoadError(false);
    } catch {
      setLoadError(true);
      setServer((s) => s ?? {});
    }
  }, []);
  useEffect(() => { void reload(); }, [reload]);

  return (
    <AreaProviders key={area.id} areaId={area.id}>
      <AreaPage area={area} section={loc.section} focus={focus} server={server} loadError={loadError} reload={reload} />
    </AreaProviders>
  );
}

function AreaPage({ area, section, focus, server, loadError, reload }: {
  area: SettingsArea; section?: string; focus: string | null; server: Record<string, ServerSetting> | null; loadError: boolean;
  reload: () => Promise<void>;
}) {
  const { total } = useDraftSummary();
  useLeaveGuard(total > 0);

  // The area's edits; AreaProviders is keyed by area, so another area starts clean.
  const [draft, setDraft] = useState<Record<string, string>>({});

  const others = useMemo(
    () => (area.id === "system" && server ? otherKeys(Object.keys(server)).map((k) => otherDef(server[k])) : []),
    [area.id, server],
  );
  const defs = useMemo(() => {
    const m = new Map<string, SettingDef>();
    for (const s of area.sections) for (const d of s.settings) m.set(d.key, d);
    for (const d of others) m.set(d.key, d);
    return m;
  }, [area, others]);

  const savedOf = (key: string) => {
    const row = server?.[key];
    return row ? (row.value ?? "") : undefined;
  };
  const changed = Object.keys(draft).filter((k) => defs.has(k) && isEdited(defs.get(k)!, draft[k], savedOf(k)));
  const errors: Record<string, string | null> = {};
  for (const k of changed) errors[k] = validateSetting(defs.get(k)!, draft[k]);

  useDraftSource("settings", {
    count: changed.length,
    validate: () => {
      const bad = changed.find((k) => errors[k]);
      return bad ? `${defs.get(bad)!.label}: ${errors[bad]}` : null;
    },
    save: async () => {
      try {
        for (const k of changed) {
          await systemApi.updateSetting(k, draft[k]);
          setDraft((d) => { const n = { ...d }; delete n[k]; return n; });
        }
        if (changed.some((k) => k.startsWith("company_"))) invalidateCompanyCache();
      } finally {
        await reload();
      }
    },
    discard: () => setDraft({}),
  });

  // Scroll to the section (and the focused setting) once the page has content.
  const ready = !!server;
  useEffect(() => {
    if (!ready) return;
    const target = (focus && document.getElementById(`setting-${focus}`)) || (section && document.getElementById(section));
    // Scroll the inner pane (#main), never the whole frame: see scrollPaneTo.
    if (target) requestAnimationFrame(() => scrollPaneTo(target, document.getElementById("main"), focus ? "center" : "start"));
  }, [ready, section, focus, area.id]);

  // Which section is on screen, for the sidebar and "On this page".
  const [active, setActive] = useState<string | undefined>(section);
  useEffect(() => {
    if (!ready || typeof IntersectionObserver === "undefined") return;
    const root = document.getElementById("main");
    const seen = new Map<string, boolean>();
    const obs = new IntersectionObserver((entries) => {
      for (const e of entries) seen.set(e.target.id, e.isIntersecting);
      const first = area.sections.find((s) => seen.get(s.id));
      if (first) setActive(first.id);
    }, { root, rootMargin: "0px 0px -60% 0px" });
    for (const s of area.sections) {
      const el = document.getElementById(s.id);
      if (el) obs.observe(el);
    }
    return () => obs.disconnect();
  }, [ready, area]);

  const row = (d: SettingDef) => (
    <SettingRow key={d.key} def={d} saved={savedOf(d.key)} draft={draft[d.key]}
      onChange={(v) => setDraft((cur) => ({ ...cur, [d.key]: v }))} error={errors[d.key]} highlight={focus === d.key} />
  );

  const other = others.length ? (
    <Card><div className="divide-y divide-line">{others.map(row)}</div></Card>
  ) : (
    <Card><EmptyState compact title="Nothing else" description="Every setting on the server has its place in a section." /></Card>
  );

  const renderSection = (s: SettingsSection) => {
    const Block = s.block ? BLOCKS[s.block] : null;
    return (
      <section key={s.id} id={s.id} aria-labelledby={`${s.id}-title`} className="scroll-mt-4 space-y-2">
        <div>
          <h2 id={`${s.id}-title`} className="text-sm font-semibold text-fg">{s.label}</h2>
          {s.description && <p className="text-[13px] text-fg-muted">{s.description}</p>}
        </div>
        {s.settings.length > 0 && (
          <Card>
            {server ? <div className="divide-y divide-line">{s.settings.map(row)}</div> : (
              <CardBody className="space-y-3"><Skeleton className="h-4 w-48" /><Skeleton className="h-9 w-full" /></CardBody>
            )}
          </Card>
        )}
        {Block && <Block server={server ?? {}} reload={reload} other={other} />}
      </section>
    );
  };

  return (
    <div className="mx-auto max-w-[1280px]">
      <PageHeader title={area.label} summary={area.description} />
      <div className="grid grid-cols-1 gap-6 lg:grid-cols-[14rem_minmax(0,1fr)] xl:grid-cols-[14rem_minmax(0,1fr)_11rem]">
        <aside className="lg:sticky lg:top-0 lg:max-h-[calc(100dvh-7rem)] lg:self-start lg:overflow-y-auto">
          <SettingsNav area={area} activeSection={active} />
        </aside>
        <div className="min-w-0">
          {loadError && (
            <p role="alert" className="mb-4 rounded-lg border border-tone-red/30 bg-tone-red-soft px-3 py-2 text-sm text-tone-red">
              Could not load the settings. Values below may be missing; reload the page to try again.
            </p>
          )}
          <div className="space-y-8">{area.sections.map(renderSection)}</div>
          <SaveBar areaLabel={area.label} />
        </div>
        <aside className="hidden xl:block xl:sticky xl:top-0 xl:self-start">
          <OnThisPage area={area} activeSection={active} />
        </aside>
      </div>
    </div>
  );
}
