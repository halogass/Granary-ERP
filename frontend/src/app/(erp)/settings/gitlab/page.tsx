"use client";

import { useEffect, useState, useCallback, useMemo } from "react";
import { localSource, rndProjectSource } from "@/lib/entitySources";
import { gitlabApi } from "@/lib/api";
import { extractApiError } from "@/lib/apiError";
import {
  GitBranch, Plus, CheckCircle2, XCircle, Activity, Eye, EyeOff, RefreshCw, ChevronDown, ChevronUp, ExternalLink, Wifi, WifiOff,
  Pencil, Search, AlertTriangle, BookOpen, Copy, KeyRound,
} from "lucide-react";
import { toast } from "@/components/ui/Toast";
import GitLabPeople from "@/components/rnd/GitLabPeople";
import {
  Button, Card, CardBody, CardHeader, Checkbox, Combobox, Dialog, EmptyState, Field, FieldGrid, IconButton, Input,
  KeyValues, PageHeader, Pill, Skeleton, StatusPill, Switch,
} from "@/components/ui";

interface GitLabConfig {
  id: string;
  name: string;
  url: string;
  api_version: string;
  is_active: boolean;
  verify_ssl: boolean;
  instance_name?: string;
  instance_version?: string;
  has_token: boolean;
  has_webhook_secret?: boolean;
}

/** A project on the GitLab server (GET /rnd/gitlab/configs/{id}/projects). */
interface GitLabProject {
  id: number;
  name: string;
  path_with_namespace: string;
  description?: string | null;
  web_url?: string;
  default_branch?: string | null;
  last_activity_at?: string;
  linked: boolean;
}

interface GitLabRepo {
  id: string;
  full_path: string;
  name: string;
  description?: string;
  web_url?: string;
  default_branch: string;
  gitlab_config_id: string;
  rnd_project_id?: string;
  gitlab_project_id: number;
  last_pipeline_status?: string;
  last_pipeline_ref?: string;
  last_pipeline_at?: string;
  is_active: boolean;
  /** 17 Nov 2026: the R&D backlog <-> GitLab issue sync */
  issue_sync?: boolean;
  issue_sync_at?: string | null;
  issue_sync_error?: string | null;
  issue_sync_imported_at?: string | null;
  issues_linked?: number;
  issues_failing?: number;
}

const BACK = { href: "/settings?area=integrations#gitlab", label: "Settings" };

/** A collapsible how-to (17 Nov 2026: the setup guides live in the page itself). */
function Guide({ title, children, defaultOpen = false }: { title: string; children: React.ReactNode; defaultOpen?: boolean }) {
  return (
    <details open={defaultOpen} className="group rounded-lg border border-line bg-surface-2/40 text-sm">
      <summary className="flex cursor-pointer list-none items-center gap-2 px-3 py-2 font-medium text-fg [&::-webkit-details-marker]:hidden">
        <BookOpen className="size-4 text-fg-muted" aria-hidden />
        <span className="flex-1">{title}</span>
        <ChevronDown className="size-4 text-fg-muted transition-transform group-open:rotate-180" aria-hidden />
      </summary>
      <div className="space-y-2 border-t border-line px-3 py-3 text-[13px] leading-relaxed text-fg-muted [&_b]:font-medium [&_b]:text-fg [&_code]:font-mono [&_code]:text-fg">
        {children}
      </div>
    </details>
  );
}

/** A random webhook secret (made in the browser; GitLab does not generate one). */
function newSecret(): string {
  const b = new Uint8Array(32);
  crypto.getRandomValues(b);
  return Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("");
}

async function copy(text: string, what: string) {
  try { await navigator.clipboard.writeText(text); toast.success(`${what} copied`); } catch { toast.error("Could not copy"); }
}

export default function GitLabSettingsPage() {
  const [configs, setConfigs] = useState<GitLabConfig[]>([]);
  const [repos, setRepos] = useState<GitLabRepo[]>([]);
  const [loading, setLoading] = useState(true);
  const [testingId, setTestingId] = useState<string | null>(null);
  const [testResults, setTestResults] = useState<Record<string, { connected: boolean; version?: string; error?: string }>>({});
  const [syncingId, setSyncingId] = useState<string | null>(null);
  const [expandedConfig, setExpandedConfig] = useState<string | null>(null);

  // New server dialog
  const [showConfigModal, setShowConfigModal] = useState(false);
  const [configForm, setConfigForm] = useState({
    name: "", url: "", access_token: "", webhook_secret: "", verify_ssl: true,
  });
  const [showToken, setShowToken] = useState(false);

  // Edit server dialog (17 Nov 2026: set the webhook secret, replace the token)
  const [editing, setEditing] = useState<GitLabConfig | null>(null);
  const [editForm, setEditForm] = useState({ name: "", access_token: "", webhook_secret: "", verify_ssl: true });
  const [showSecret, setShowSecret] = useState(false);

  // New repo dialog
  const [showRepoModal, setShowRepoModal] = useState(false);
  const EMPTY_REPO = {
    gitlab_config_id: "", gitlab_project_id: "", full_path: "",
    name: "", web_url: "", default_branch: "main", description: "", rnd_project_id: "",
  };
  const [repoForm, setRepoForm] = useState(EMPTY_REPO);
  // 17 Nov 2026: the server's projects are listed to pick from (typing them in stays possible)
  const [projects, setProjects] = useState<GitLabProject[]>([]);
  const [projectsLoading, setProjectsLoading] = useState(false);
  const [projectsError, setProjectsError] = useState<string | null>(null);
  const [projectSearch, setProjectSearch] = useState("");
  const [manual, setManual] = useState(false);

  const [saving, setSaving] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [configsRes, reposRes] = await Promise.all([
        gitlabApi.listConfigs(),
        gitlabApi.listRepos(),
      ]);
      setConfigs(configsRes.data ?? []);
      setRepos(reposRes.data ?? []);
    } catch { setConfigs([]); setRepos([]); } finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  // With one server, the repository dialog starts on it.
  function openRepoModal() {
    setRepoForm({ ...EMPTY_REPO, gitlab_config_id: configs.length === 1 ? configs[0].id : "" });
    setProjectSearch(""); setManual(false);
    setShowRepoModal(true);
  }

  // Fetch the server's projects when the dialog is open (search debounced).
  const serverId = repoForm.gitlab_config_id;
  useEffect(() => {
    if (!showRepoModal || !serverId) { setProjects([]); return; }
    let cancelled = false;
    const t = setTimeout(async () => {
      setProjectsLoading(true); setProjectsError(null);
      try {
        const res = await gitlabApi.listProjects(serverId, projectSearch.trim() || undefined);
        if (!cancelled) setProjects(res.data ?? []);
      } catch (e) {
        if (!cancelled) { setProjects([]); setProjectsError(extractApiError(e, "Could not list the server's projects")); }
      } finally { if (!cancelled) setProjectsLoading(false); }
    }, projectSearch ? 300 : 0);
    return () => { cancelled = true; clearTimeout(t); };
  }, [showRepoModal, serverId, projectSearch]);

  function pickProject(p: GitLabProject) {
    setRepoForm((f) => ({
      ...f, gitlab_project_id: String(p.id), full_path: p.path_with_namespace, name: p.name,
      web_url: p.web_url ?? "", default_branch: p.default_branch || "main", description: p.description ?? "",
    }));
  }

  function openEdit(c: GitLabConfig) {
    setEditing(c);
    setEditForm({ name: c.name, access_token: "", webhook_secret: "", verify_ssl: c.verify_ssl });
    setShowSecret(false);
  }

  async function saveEdit() {
    if (!editing || !editForm.name.trim()) return;
    setSaving(true);
    try {
      const body: Record<string, unknown> = { name: editForm.name.trim(), verify_ssl: editForm.verify_ssl };
      if (editForm.access_token.trim()) body.access_token = editForm.access_token.trim();
      if (editForm.webhook_secret.trim()) body.webhook_secret = editForm.webhook_secret.trim();
      await gitlabApi.updateConfig(editing.id, body);
      toast.success("Server saved");
      setEditing(null);
      await load();
    } catch (e) { toast.error(extractApiError(e)); } finally { setSaving(false); }
  }

  const [issueSyncing, setIssueSyncing] = useState<string | null>(null);
  const [confirmSync, setConfirmSync] = useState<GitLabRepo | null>(null);

  async function setIssueSync(repo: GitLabRepo, on: boolean) {
    try {
      await gitlabApi.updateRepo(repo.id, { issue_sync: on });
      toast.success(on ? "Issue sync on: the first sync imports both ways within a minute" : "Issue sync off");
      setConfirmSync(null);
      await load();
    } catch (e) { toast.error(extractApiError(e)); }
  }

  async function syncIssues(repoId: string) {
    setIssueSyncing(repoId);
    try {
      const res = await gitlabApi.syncIssues(repoId);
      const d = res.data ?? {};
      if (d.error) toast.error(d.error);
      else if (d.imported !== undefined) toast.success(`Imported: ${d.imported} issue(s) became items, ${d.created} item(s) got an issue`);
      else toast.success(`Synced: ${d.pushed ?? 0} item(s) sent, ${d.pulled ?? 0} issue(s) read, ${d.comments ?? 0} comment(s)${d.errors ? `, ${d.errors} error(s)` : ""}`);
      await load();
    } catch (e) { toast.error(extractApiError(e)); } finally { setIssueSyncing(null); }
  }

  async function linkProject(repoId: string, rndProjectId: string) {
    try {
      await gitlabApi.updateRepo(repoId, { rnd_project_id: rndProjectId || null });
      setRepos((prev) => prev.map((r) => (r.id === repoId ? { ...r, rnd_project_id: rndProjectId || undefined } : r)));
      toast.success(rndProjectId ? "Linked to the R&D project" : "Unlinked from the R&D project");
    } catch (e) { toast.error(extractApiError(e)); }
  }

  const configOptions = useMemo(() => localSource(configs.map((c) => ({
    id: c.id, label: c.name, sublabel: c.url,
  }))), [configs]);

  async function createConfig() {
    if (!configForm.name.trim() || !configForm.url.trim() || !configForm.access_token.trim()) return;
    setSaving(true);
    try {
      await gitlabApi.createConfig({
        name: configForm.name.trim(),
        url: configForm.url.trim(),
        access_token: configForm.access_token.trim(),
        webhook_secret: configForm.webhook_secret.trim() || undefined,
        verify_ssl: configForm.verify_ssl,
      });
      setShowConfigModal(false);
      setConfigForm({ name: "", url: "", access_token: "", webhook_secret: "", verify_ssl: true });
      await load();
    } catch (e) { toast.error(extractApiError(e)); } finally { setSaving(false); }
  }

  async function addRepo() {
    if (!repoForm.full_path.trim() || !repoForm.name.trim() || !repoForm.gitlab_config_id || !repoForm.gitlab_project_id) return;
    setSaving(true);
    try {
      await gitlabApi.addRepo({
        gitlab_config_id: repoForm.gitlab_config_id,
        gitlab_project_id: parseInt(repoForm.gitlab_project_id),
        full_path: repoForm.full_path.trim(),
        name: repoForm.name.trim(),
        web_url: repoForm.web_url.trim() || undefined,
        default_branch: repoForm.default_branch || "main",
        description: repoForm.description.trim() || undefined,
        rnd_project_id: repoForm.rnd_project_id || undefined,
      });
      setShowRepoModal(false);
      setRepoForm(EMPTY_REPO);
      toast.success("Repository added");
      await load();
    } catch (e) { toast.error(extractApiError(e)); } finally { setSaving(false); }
  }

  async function testConnection(id: string) {
    setTestingId(id);
    try {
      const res = await gitlabApi.testConfig(id);
      setTestResults((prev) => ({ ...prev, [id]: res.data }));
    } catch {
      setTestResults((prev) => ({ ...prev, [id]: { connected: false, error: "Request failed" } }));
    } finally { setTestingId(null); }
  }

  async function syncPipelines(repoId: string) {
    setSyncingId(repoId);
    try {
      await gitlabApi.syncPipelines(repoId);
      await load();
    } finally { setSyncingId(null); }
  }

  const header = (
    <PageHeader title="GitLab integration" back={BACK} summary="Connect your self-hosted GitLab server to Granary"
      actions={<>
        <Button onClick={openRepoModal}><GitBranch aria-hidden /> Add repository</Button>
        <Button variant="primary" onClick={() => setShowConfigModal(true)}><Plus aria-hidden /> Add server</Button>
      </>} />
  );

  if (loading) {
    return (
      <div className="mx-auto max-w-4xl">
        {header}
        <div className="space-y-3" aria-busy>
          <Skeleton className="h-16 w-full" />
          <Skeleton className="h-16 w-full" />
          <p className="text-sm text-fg-muted">Loading GitLab configuration...</p>
        </div>
      </div>
    );
  }

  return (
    <div className="mx-auto max-w-4xl">
      {header}

      <div className="space-y-4">
        <Guide title="How this works" defaultOpen={repos.length === 0}>
          <p>The ERP reads your GitLab server so R&D projects show their repositories and build (pipeline) status.</p>
          <ol className="list-decimal space-y-1 pl-5">
            <li><b>Server:</b> add the GitLab server once with a personal access token (scope <code>api</code>). In GitLab: your avatar &gt; <b>Edit profile</b> &gt; <b>Access tokens</b> &gt; Add new token.</li>
            <li><b>Webhook secret:</b> open the server below (<b>Edit</b>), press <b>Generate</b>, save, and copy the secret. You will paste the same value into GitLab.</li>
            <li><b>Repositories:</b> press <b>Add repository</b>, pick the project from the server&apos;s list, and choose its R&amp;D project.</li>
            <li><b>Webhook in GitLab:</b> in each project, <b>Settings &gt; Webhooks &gt; Add new webhook</b>, with the URL and secret from the server&apos;s details below (steps there).</li>
            <li><b>Check:</b> press <b>Sync</b> on a repository to pull its recent pipelines; afterwards GitLab reports new ones by itself.</li>
          </ol>
        </Guide>

        {/* Servers */}
        <Card>
          <CardHeader title="GitLab servers" />
          {configs.length === 0 ? (
            <EmptyState icon={GitBranch} title="No GitLab server connected"
              description="Connect your self-hosted GitLab to track pipelines and issues."
              action={<Button variant="primary" onClick={() => setShowConfigModal(true)}><Plus aria-hidden /> Add server</Button>} />
          ) : (
            <ul className="divide-y divide-line">
              {configs.map((config) => {
                const testResult = testResults[config.id];
                const repoCount = repos.filter((r) => r.gitlab_config_id === config.id).length;
                const isExpanded = expandedConfig === config.id;
                return (
                  <li key={config.id}>
                    <div className="flex flex-wrap items-center gap-3 px-4 py-3">
                      <span className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-surface-2 text-fg-muted">
                        {config.is_active ? <Wifi className="size-4 text-tone-blue" aria-label="Active" /> : <WifiOff className="size-4" aria-label="Inactive" />}
                      </span>
                      <div className="min-w-0 flex-1">
                        <p className="text-sm font-medium text-fg">{config.name}</p>
                        <p className="truncate font-mono text-xs text-fg-muted">{config.url}</p>
                      </div>
                      {testResult && (
                        testResult.connected
                          ? <Pill tone="blue" icon={CheckCircle2}>{testResult.version}</Pill>
                          : <Pill tone="red" icon={XCircle}>{testResult.error}</Pill>
                      )}
                      <span className="text-xs text-fg-muted tabular">{repoCount} repo{repoCount !== 1 ? "s" : ""}</span>
                      {config.instance_version && (
                        <span className="rounded border border-line bg-surface-2 px-1.5 font-mono text-xs text-fg-muted">v{config.instance_name}</span>
                      )}
                      {!config.has_webhook_secret && (
                        <Pill tone="orange" icon={AlertTriangle} title="GitLab webhooks are refused until a webhook secret is set">No webhook secret</Pill>
                      )}
                      <Button size="sm" onClick={() => openEdit(config)}><Pencil aria-hidden /> Edit</Button>
                      <Button size="sm" onClick={() => testConnection(config.id)} disabled={testingId === config.id}>
                        {testingId === config.id ? <RefreshCw className="animate-spin" aria-hidden /> : <Activity aria-hidden />} Test
                      </Button>
                      <IconButton size="sm" label={isExpanded ? "Hide details" : "Show details"} icon={isExpanded ? ChevronUp : ChevronDown}
                        aria-expanded={isExpanded} onClick={() => setExpandedConfig(isExpanded ? null : config.id)} />
                    </div>
                    {isExpanded && (
                      <div className="space-y-4 border-t border-line bg-surface-2/50 px-4 py-3">
                        <KeyValues cols={3} items={[
                          { label: "API version", value: <span className="font-mono">{config.api_version}</span> },
                          { label: "SSL verification", value: config.verify_ssl ? "Enabled" : "Disabled" },
                          { label: "Access token", value: config.has_token ? "Configured" : "Not set" },
                          { label: "Webhook secret", value: config.has_webhook_secret ? "Set" : <span className="text-tone-orange">Not set: webhooks are refused</span> },
                        ]} />
                        <div>
                          <p className="mb-1 text-[13px] text-fg-muted">Webhook URL</p>
                          <div className="flex items-center gap-2">
                            <code className="block min-w-0 flex-1 truncate rounded-lg border border-line bg-surface px-3 py-2 font-mono text-xs text-fg">
                              {window.location.origin}/api/v1/rnd/gitlab/webhook
                            </code>
                            <IconButton size="sm" label="Copy webhook URL" icon={Copy}
                              onClick={() => copy(`${window.location.origin}/api/v1/rnd/gitlab/webhook`, "Webhook URL")} />
                          </div>
                        </div>
                        <Guide title="Add the webhook in GitLab">
                          <ol className="list-decimal space-y-1 pl-5">
                            <li>In GitLab, open the project, then <b>Settings &gt; Webhooks &gt; Add new webhook</b>.</li>
                            <li><b>URL:</b> the webhook URL above.</li>
                            <li><b>Secret token:</b> the same secret as this server&apos;s webhook secret. GitLab does not make one: generate it here with <b>Edit &gt; Generate</b>, save, and paste it in GitLab. One secret serves every project on this server.</li>
                            <li><b>Trigger:</b> tick Push events, Tag push events, Comments, Merge request events, Issues events and Pipeline events.</li>
                            <li>Keep <b>Enable SSL verification</b> on, then <b>Add webhook</b>.</li>
                            <li>Check it with <b>Test &gt; Pipeline events</b> on the webhook: GitLab should show HTTP 200. A 401 means the secrets differ (or none is set here).</li>
                          </ol>
                          <p>Do this in every project you link below. A group webhook (group <b>Settings &gt; Webhooks</b>) covers all its projects at once, if your GitLab edition offers it.</p>
                        </Guide>
                      </div>
                    )}
                  </li>
                );
              })}
            </ul>
          )}
        </Card>

        {/* Repositories */}
        <Card>
          <CardHeader title="Linked repositories" />
          {repos.length === 0 ? (
            <EmptyState compact title="No repositories linked yet."
              action={<Button onClick={openRepoModal}><Plus aria-hidden /> Add a repository</Button>} />
          ) : (
            <ul className="divide-y divide-line">
              {repos.map((repo) => {
                const ps = repo.last_pipeline_status;
                return (
                  <li key={repo.id} className="flex flex-wrap items-center gap-3 px-4 py-3">
                    <span className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-surface-2 text-fg-muted">
                      <GitBranch className="size-4" aria-hidden />
                    </span>
                    <div className="min-w-0 flex-1">
                      <div className="flex flex-wrap items-center gap-2">
                        <p className="text-sm font-medium text-fg">{repo.name}</p>
                        <span className="font-mono text-xs text-fg-muted">{repo.full_path}</span>
                      </div>
                      <div className="mt-0.5 flex flex-wrap items-center gap-2 text-xs text-fg-muted">
                        <span className="font-mono">#{repo.gitlab_project_id}</span>
                        <span>branch: <span className="font-mono">{repo.default_branch}</span></span>
                        {!repo.is_active && <StatusPill status="inactive" />}
                      </div>
                    </div>
                    <div className="w-56">
                      <Combobox value={repo.rnd_project_id ?? ""} source={rndProjectSource} placeholder="R&D project"
                        emptyLabel="No R&D projects" aria-label={`R&D project for ${repo.name}`}
                        onChange={(id) => linkProject(repo.id, id)} />
                    </div>
                    {ps && (
                      <StatusPill status={ps} tone={ps === "passed" ? "blue" : undefined}
                        label={<>{ps}{repo.last_pipeline_ref && <span className="opacity-70"> ({repo.last_pipeline_ref})</span>}</>} />
                    )}
                    <div className="flex basis-full flex-wrap items-center gap-3 pl-11 text-xs text-fg-muted">
                      <Switch checked={!!repo.issue_sync} label="Sync backlog with GitLab issues" disabled={!repo.rnd_project_id}
                        onCheckedChange={(on) => (on && !repo.issue_sync_imported_at ? setConfirmSync(repo) : setIssueSync(repo, on))} />
                      {!repo.rnd_project_id && <span>Pick an R&amp;D project first.</span>}
                      {repo.issue_sync && (
                        <>
                          <span>{repo.issues_linked ?? 0} item(s) linked</span>
                          <span>{repo.issue_sync_at ? `last sync ${new Date(repo.issue_sync_at).toLocaleString("en-GB", { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" })}` : "waiting for the first sync"}</span>
                          {(repo.issues_failing ?? 0) > 0 && <Pill tone="orange" icon={AlertTriangle}>{repo.issues_failing} item(s) failing</Pill>}
                          <Button size="sm" variant="ghost" onClick={() => syncIssues(repo.id)} disabled={issueSyncing === repo.id}>
                            <RefreshCw className={issueSyncing === repo.id ? "animate-spin" : undefined} aria-hidden /> Sync issues now
                          </Button>
                        </>
                      )}
                      {repo.issue_sync && repo.issue_sync_error && (
                        <p className="basis-full text-tone-orange">Last error: {repo.issue_sync_error}</p>
                      )}
                    </div>
                    <Button size="sm" onClick={() => syncPipelines(repo.id)} disabled={syncingId === repo.id}>
                      <RefreshCw className={syncingId === repo.id ? "animate-spin" : undefined} aria-hidden /> Sync
                    </Button>
                    {repo.web_url && (
                      <Button size="sm" variant="ghost" asChild>
                        <a href={repo.web_url} target="_blank" rel="noreferrer" aria-label={`Open ${repo.name} in GitLab`} title="Open in GitLab">
                          <ExternalLink aria-hidden />
                        </a>
                      </Button>
                    )}
                  </li>
                );
              })}
            </ul>
          )}
        </Card>

        <Guide title="Backlog and GitLab issues (two-way sync)">
          <p>With <b>Sync backlog with GitLab issues</b> on, a repository&apos;s R&amp;D project and its GitLab issues stay in step, both ways, about once a minute.</p>
          <ul className="list-disc space-y-1 pl-5">
            <li><b>First sync:</b> every GitLab issue becomes a backlog item and every backlog item gets an issue (Done items as closed issues). You get a notice with the numbers.</li>
            <li><b>What follows:</b> title, description with acceptance criteria, status, priority, type, labels, assignee and comments. Story points, sprint, epic and board order stay in the ERP.</li>
            <li><b>Status:</b> Done closes the issue; the other statuses show as labels like <code>status::in_progress</code>, which a GitLab issue board can use as columns.</li>
            <li><b>Both sides changed:</b> the newer edit wins; the other value is kept in the item&apos;s history.</li>
            <li><b>Deleting:</b> an item deleted here closes its issue with the label <code>erp::deleted</code>; an issue deleted in GitLab leaves the item here, unlinked.</li>
            <li><b>Webhook:</b> tick <b>Issues events</b> and <b>Comments</b> in the GitLab webhook so changes arrive at once (without it they arrive with the next pull).</li>
            <li><b>People:</b> matched by e-mail where GitLab shows it; set the rest below. Comments from here are posted by the access token&apos;s account as &quot;<b>Name (ERP):</b>&quot;.</li>
          </ul>
        </Guide>

        {configs.filter((c) => repos.some((r) => r.gitlab_config_id === c.id && r.issue_sync)).map((c) => (
          <GitLabPeople key={c.id} configId={c.id} serverName={c.name} />
        ))}
      </div>

      {/* Turn on the issue sync (17 Nov 2026) */}
      <Dialog open={!!confirmSync} onOpenChange={(o) => { if (!o) setConfirmSync(null); }} title="Turn on the issue sync?"
        footer={<>
          <Button onClick={() => setConfirmSync(null)}>Cancel</Button>
          <Button variant="primary" onClick={() => confirmSync && setIssueSync(confirmSync, true)}>Turn on and import</Button>
        </>}>
        <p className="text-sm text-fg">
          The first sync imports both ways: every issue of <b>{confirmSync?.full_path}</b> becomes a backlog item, and every backlog
          item of its R&amp;D project gets a GitLab issue (Done items as closed issues). After that, changes on either side follow.
        </p>
      </Dialog>

      {/* Add server */}
      <Dialog open={showConfigModal} onOpenChange={setShowConfigModal} title="Add GitLab server"
        footer={<>
          <Button onClick={() => setShowConfigModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={createConfig} loading={saving}
            disabled={!configForm.name.trim() || !configForm.url.trim() || !configForm.access_token.trim()}>Add server</Button>
        </>}>
        <div className="space-y-4">
          <Field label="Display name" required>
            <Input value={configForm.name} onChange={(e) => setConfigForm({ ...configForm, name: e.target.value })} placeholder="Company GitLab" />
          </Field>
          <Field label="GitLab URL" required>
            <Input value={configForm.url} onChange={(e) => setConfigForm({ ...configForm, url: e.target.value })}
              placeholder="https://gitlab.example.com" className="font-mono" />
          </Field>
          <Field label="Personal access token" required
            hint={<>Requires <code className="font-mono">api</code> or <code className="font-mono">read_api</code> scope.</>}>
            <div className="relative">
              <Input type={showToken ? "text" : "password"} value={configForm.access_token}
                onChange={(e) => setConfigForm({ ...configForm, access_token: e.target.value })}
                placeholder="glpat-xxxxxxxxxxxxxxxxxxxx" className="pr-11 font-mono" autoComplete="off" />
              <IconButton size="sm" label={showToken ? "Hide token" : "Show token"} icon={showToken ? EyeOff : Eye}
                onClick={() => setShowToken(!showToken)} className="absolute right-0.5 top-1/2 -translate-y-1/2" />
            </div>
          </Field>
          <Field label="Webhook secret" hint="GitLab webhooks are refused without one. Paste the same value into each GitLab webhook's Secret token.">
            <div className="flex gap-2">
              <Input value={configForm.webhook_secret} className="font-mono" autoComplete="off"
                onChange={(e) => setConfigForm({ ...configForm, webhook_secret: e.target.value })} />
              <Button onClick={() => setConfigForm({ ...configForm, webhook_secret: newSecret() })}><KeyRound aria-hidden /> Generate</Button>
            </div>
          </Field>
          <Checkbox checked={configForm.verify_ssl} label="Verify SSL certificate"
            onChange={(e) => setConfigForm({ ...configForm, verify_ssl: e.target.checked })} />
        </div>
      </Dialog>

      {/* Add repository */}
      <Dialog open={showRepoModal} onOpenChange={setShowRepoModal} title="Add repository"
        footer={<>
          <Button onClick={() => setShowRepoModal(false)}>Cancel</Button>
          <Button variant="primary" onClick={addRepo} loading={saving}
            disabled={!repoForm.full_path.trim() || !repoForm.name.trim() || !repoForm.gitlab_config_id || !repoForm.gitlab_project_id}>Add repository</Button>
        </>}>
        <div className="space-y-4">
          <Field label="GitLab server" required>
            <Combobox value={repoForm.gitlab_config_id} source={configOptions}
              placeholder="Select server" emptyLabel="No servers configured"
              onChange={(id) => setRepoForm({ ...EMPTY_REPO, gitlab_config_id: id })} />
          </Field>

          {repoForm.gitlab_config_id && !manual && (
            <Field label="Project" required hint="The projects this server's access token can see, most recently active first.">
              <div className="rounded-lg border border-line">
                <div className="border-b border-line p-2">
                  <Input type="search" leading={Search} value={projectSearch} placeholder="Search projects" aria-label="Search projects"
                    autoComplete="off" data-1p-ignore data-lpignore="true" data-form-type="other"
                    onChange={(e) => setProjectSearch(e.target.value)} />
                </div>
                <ul className="max-h-64 overflow-y-auto" role="listbox" aria-label="GitLab projects">
                  {projectsLoading && <li className="px-3 py-3 text-sm text-fg-muted">Loading projects...</li>}
                  {!projectsLoading && projectsError && <li className="px-3 py-3 text-sm text-tone-red">{projectsError}</li>}
                  {!projectsLoading && !projectsError && projects.length === 0 && (
                    <li className="px-3 py-3 text-sm text-fg-muted">No projects found{projectSearch ? ` for "${projectSearch}"` : ""}.</li>
                  )}
                  {!projectsLoading && projects.map((p) => {
                    const selected = repoForm.gitlab_project_id === String(p.id);
                    return (
                      <li key={p.id}>
                        <button type="button" role="option" aria-selected={selected} disabled={p.linked}
                          onClick={() => pickProject(p)}
                          className={`flex w-full items-start gap-2 px-3 py-2 text-left text-sm hover:bg-surface-2 disabled:cursor-not-allowed disabled:opacity-50 ${selected ? "bg-surface-2" : ""}`}>
                          <GitBranch className="mt-0.5 size-4 shrink-0 text-fg-muted" aria-hidden />
                          <span className="min-w-0 flex-1">
                            <span className="block font-medium text-fg">{p.name}</span>
                            <span className="block truncate font-mono text-xs text-fg-muted">{p.path_with_namespace}</span>
                          </span>
                          {p.linked ? <span className="text-xs text-fg-muted">Linked</span> : selected ? <CheckCircle2 className="size-4 text-tone-blue" aria-hidden /> : null}
                        </button>
                      </li>
                    );
                  })}
                </ul>
              </div>
              <button type="button" className="mt-1 text-xs text-fg-muted underline" onClick={() => setManual(true)}>Enter the project by hand instead</button>
            </Field>
          )}

          {repoForm.gitlab_config_id && manual && (
            <>
              <FieldGrid cols={2}>
                <Field label="GitLab project ID" required hint="On the project's GitLab page, under its name.">
                  <Input type="number" value={repoForm.gitlab_project_id} placeholder="42" className="font-mono"
                    onChange={(e) => setRepoForm({ ...repoForm, gitlab_project_id: e.target.value })} />
                </Field>
                <Field label="Default branch">
                  <Input value={repoForm.default_branch} placeholder="main" className="font-mono"
                    onChange={(e) => setRepoForm({ ...repoForm, default_branch: e.target.value })} />
                </Field>
              </FieldGrid>
              <Field label="Full path" required>
                <Input value={repoForm.full_path} placeholder="group/project" className="font-mono"
                  onChange={(e) => setRepoForm({ ...repoForm, full_path: e.target.value })} />
              </Field>
              <Field label="Web URL">
                <Input value={repoForm.web_url} placeholder="https://gitlab.example.com/products/billing-service" className="font-mono"
                  onChange={(e) => setRepoForm({ ...repoForm, web_url: e.target.value })} />
              </Field>
              <button type="button" className="text-xs text-fg-muted underline" onClick={() => setManual(false)}>Pick from the server&apos;s list instead</button>
            </>
          )}

          {repoForm.gitlab_project_id && (
            <FieldGrid cols={2}>
              <Field label="Display name" required>
                <Input value={repoForm.name} placeholder="Billing service" onChange={(e) => setRepoForm({ ...repoForm, name: e.target.value })} />
              </Field>
              <Field label="R&D project" hint="Shows this repository and its builds on the project.">
                <Combobox value={repoForm.rnd_project_id} source={rndProjectSource} placeholder="None"
                  emptyLabel="No R&D projects" onChange={(id) => setRepoForm({ ...repoForm, rnd_project_id: id })} />
              </Field>
            </FieldGrid>
          )}
        </div>
      </Dialog>

      {/* Edit server (17 Nov 2026) */}
      <Dialog open={!!editing} onOpenChange={(o) => { if (!o) setEditing(null); }} title={`Edit ${editing?.name ?? "server"}`}
        footer={<>
          <Button onClick={() => setEditing(null)}>Cancel</Button>
          <Button variant="primary" onClick={saveEdit} loading={saving} disabled={!editForm.name.trim()}>Save</Button>
        </>}>
        <div className="space-y-4">
          <Field label="Display name" required>
            <Input value={editForm.name} onChange={(e) => setEditForm({ ...editForm, name: e.target.value })} />
          </Field>
          <Field label="Webhook secret"
            hint={editing?.has_webhook_secret
              ? "A secret is set (never shown again). Fill this only to replace it, then update every GitLab webhook to match."
              : "Not set: GitLab webhooks are refused. Generate one, save, then paste it as the Secret token in each GitLab webhook."}>
            <div className="flex gap-2">
              <div className="relative flex-1">
                <Input type={showSecret ? "text" : "password"} value={editForm.webhook_secret} className="pr-11 font-mono" autoComplete="off"
                  placeholder={editing?.has_webhook_secret ? "Leave empty to keep the current secret" : ""}
                  onChange={(e) => setEditForm({ ...editForm, webhook_secret: e.target.value })} />
                <IconButton size="sm" label={showSecret ? "Hide secret" : "Show secret"} icon={showSecret ? EyeOff : Eye}
                  onClick={() => setShowSecret(!showSecret)} className="absolute right-0.5 top-1/2 -translate-y-1/2" />
              </div>
              <Button onClick={() => { setEditForm({ ...editForm, webhook_secret: newSecret() }); setShowSecret(true); }}><KeyRound aria-hidden /> Generate</Button>
              {editForm.webhook_secret && <IconButton label="Copy secret" icon={Copy} onClick={() => copy(editForm.webhook_secret, "Secret")} />}
            </div>
          </Field>
          <Field label="Personal access token" hint={<>Leave empty to keep the current token. A new one needs the <code className="font-mono">api</code> scope.</>}>
            <Input type="password" value={editForm.access_token} placeholder="glpat-..." className="font-mono" autoComplete="off"
              onChange={(e) => setEditForm({ ...editForm, access_token: e.target.value })} />
          </Field>
          <Checkbox checked={editForm.verify_ssl} label="Verify SSL certificate"
            onChange={(e) => setEditForm({ ...editForm, verify_ssl: e.target.checked })} />
        </div>
      </Dialog>
    </div>
  );
}
