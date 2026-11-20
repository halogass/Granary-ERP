"use client";

import { useEffect, useRef, useState } from "react";
import MyEmailSignature from "@/components/settings/MyEmailSignature";
import MyWhatsAppSignature from "@/components/settings/MyWhatsAppSignature";
import MyWhatsAppChats from "@/components/nary/MyWhatsAppChats";
import TwoFactorSection from "@/components/settings/TwoFactorSection";
import { authApi } from "@/lib/api";
import { Lock, Save, Eye, EyeOff, CheckCircle, AlertCircle, ImagePlus, Info, Trash2 } from "lucide-react";
import SignaturePad from "@/components/sign/SignaturePad";
import { useDropToInput, isImage } from "@/hooks/useFileDrop";
import {
  Button, Card, CardBody, CardFooter, CardHeader, Field, FieldGrid, IconButton, Input, PageHeader, Pill, Skeleton,
} from "@/components/ui";
import { cn } from "@/lib/utils";

type Result = { ok: boolean; msg: string } | null;

function ResultNote({ result, inline }: { result: Result; inline?: boolean }) {
  if (!result) return null;
  const Icon = result.ok ? CheckCircle : AlertCircle;
  return (
    <div role={result.ok ? "status" : "alert"} className={cn("flex items-center gap-2 text-sm",
      inline ? "text-xs font-medium" : "rounded-lg px-3 py-2.5",
      result.ok ? (inline ? "text-tone-blue" : "bg-tone-blue-soft text-tone-blue") : (inline ? "text-tone-red" : "bg-tone-red-soft text-tone-red"))}>
      <Icon className="size-4 shrink-0" aria-hidden /> {result.msg}
    </div>
  );
}

/** A value only HR can change, shown like a disabled input. */
function ReadOnly({ value }: { value?: string | null }) {
  return (
    <div className="flex h-11 items-center justify-between gap-2 rounded-lg border border-line bg-surface-2 px-3 text-sm text-fg md:h-9">
      <span className="truncate">{value ?? "-"}</span>
      <span className="flex shrink-0 items-center gap-1 text-xs text-fg-subtle"><Info className="size-3" aria-hidden /> HR only</span>
    </div>
  );
}

interface ProfileData {
  full_name: string;
  email: string;
  phone: string | null;
  department: string | null;
  position: string | null;
  avatar_url: string | null;
  is_superadmin: boolean;
  is_system_admin: boolean;
  portal_type: string;
}

export default function ProfilePage() {
  const [profile, setProfile] = useState<ProfileData | null>(null);
  const [loading, setLoading] = useState(true);

  const [phone, setPhone] = useState("");
  const [saving, setSaving] = useState(false);
  const [saveResult, setSaveResult] = useState<Result>(null);

  const [avatarPreview, setAvatarPreview] = useState<string | null>(null);
  const [uploadingAvatar, setUploadingAvatar] = useState(false);
  const [avatarResult, setAvatarResult] = useState<Result>(null);
  const avatarInputRef = useRef<HTMLInputElement>(null);
  const { dragActive: avatarDrag, dropHandlers: avatarDrop } = useDropToInput(avatarInputRef, { accept: isImage, single: true });

  const [pwForm, setPwForm] = useState({ current_password: "", new_password: "", confirm: "" });
  const [showCurrent, setShowCurrent] = useState(false);
  const [showNew, setShowNew] = useState(false);
  const [changingPw, setChangingPw] = useState(false);
  const [pwResult, setPwResult] = useState<Result>(null);

  const [sig, setSig] = useState<string | null>(null);          // current pad value
  const [savedSig, setSavedSig] = useState<string | null>(null); // persisted value
  const [savingSig, setSavingSig] = useState(false);
  const [sigResult, setSigResult] = useState<Result>(null);

  useEffect(() => {
    async function load() {
      try {
        const res = await authApi.me();
        const d = res.data;
        setProfile(d);
        setPhone(d.phone ?? "");
        setAvatarPreview(d.avatar_url ?? null);
        try {
          const sg = await authApi.getSignature();
          setSavedSig(sg.data.signature_data ?? null);
          setSig(sg.data.signature_data ?? null);
        } catch { /* none */ }
      } finally { setLoading(false); }
    }
    load();
  }, []);

  async function saveSignature() {
    if (!sig) { setSigResult({ ok: false, msg: "Draw or upload a signature first" }); return; }
    setSavingSig(true); setSigResult(null);
    try {
      await authApi.saveSignature(sig);
      setSavedSig(sig);
      setSigResult({ ok: true, msg: "Signature saved" });
    } catch (err: unknown) {
      setSigResult({ ok: false, msg: (err as { response?: { data?: { detail?: string } } })?.response?.data?.detail ?? "Could not save" });
    } finally { setSavingSig(false); }
  }
  async function removeSignature() {
    try {
      await authApi.deleteSignature();
      setSavedSig(null); setSig(null);
      setSigResult({ ok: true, msg: "Signature removed" });
    } catch { setSigResult({ ok: false, msg: "Could not remove" }); }
  }

  async function handleSaveProfile() {
    setSaving(true); setSaveResult(null);
    try {
      await authApi.updateMe({ phone: phone || undefined });
      setSaveResult({ ok: true, msg: "Phone updated successfully" });
    } catch (err: unknown) {
      setSaveResult({ ok: false, msg: (err as { response?: { data?: { detail?: string } } })?.response?.data?.detail ?? "Update failed" });
    } finally { setSaving(false); }
  }

  async function handleAvatarChange(e: React.ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0];
    if (!file) return;
    const preview = URL.createObjectURL(file);
    setAvatarPreview(preview);
    setAvatarResult(null);
    setUploadingAvatar(true);
    try {
      const fd = new FormData();
      fd.append("file", file);
      const res = await authApi.uploadAvatar(fd);
      setAvatarPreview(res.data.avatar_url);
      setProfile((p) => p ? { ...p, avatar_url: res.data.avatar_url } : p);
      setAvatarResult({ ok: true, msg: "Photo updated" });
    } catch (err: unknown) {
      setAvatarPreview(profile?.avatar_url ?? null);
      setAvatarResult({ ok: false, msg: (err as { response?: { data?: { detail?: string } } })?.response?.data?.detail ?? "Upload failed" });
    } finally {
      setUploadingAvatar(false);
      e.target.value = "";
    }
  }

  async function handleChangePassword() {
    if (!pwForm.current_password || !pwForm.new_password) {
      setPwResult({ ok: false, msg: "Fill in all password fields" }); return;
    }
    if (pwForm.new_password.length < 8) {
      setPwResult({ ok: false, msg: "New password must be at least 8 characters" }); return;
    }
    if (pwForm.new_password !== pwForm.confirm) {
      setPwResult({ ok: false, msg: "Passwords do not match" }); return;
    }
    setChangingPw(true); setPwResult(null);
    try {
      await authApi.changePassword({
        current_password: pwForm.current_password,
        new_password: pwForm.new_password,
      });
      setPwForm({ current_password: "", new_password: "", confirm: "" });
      // The backend ends every other session on a password change; this one stays signed in.
      setPwResult({ ok: true, msg: "Password changed. Every other device has been signed out." });
    } catch (err: unknown) {
      setPwResult({ ok: false, msg: (err as { response?: { data?: { detail?: string } } })?.response?.data?.detail ?? "Failed to change password" });
    } finally { setChangingPw(false); }
  }

  if (loading) {
    return (
      <div className="max-w-2xl space-y-4" aria-label="Loading profile">
        <Skeleton className="h-8 w-48" /><Skeleton className="h-56 w-full" /><Skeleton className="h-40 w-full" />
      </div>
    );
  }

  const roleLabel = profile?.is_superadmin ? "Superadmin" : profile?.is_system_admin ? "System admin" : "Staff";
  const initials = (profile?.full_name ?? "?").split(" ").map((w) => w[0]).slice(0, 2).join("").toUpperCase();
  const mismatch = !!pwForm.confirm && pwForm.confirm !== pwForm.new_password;

  return (
    <div className="max-w-2xl">
      <PageHeader title="My profile" summary="Your contact info and account security" />

      <div className="space-y-4">
        <Card>
          <CardBody className="space-y-5">
            <div className="flex items-center gap-5">
              <div className="relative shrink-0" {...avatarDrop} title="Drag and drop a photo, or use the photo button">
                <div className={cn("flex size-20 items-center justify-center overflow-hidden rounded-lg bg-surface-2 transition-shadow",
                  avatarDrag && "ring-2 ring-brand ring-offset-2 ring-offset-surface")}>
                  {avatarPreview ? (
                    // eslint-disable-next-line @next/next/no-img-element
                    <img src={avatarPreview} alt="Your photo" className="h-full w-full object-cover" />
                  ) : (
                    <span className="text-2xl font-semibold text-fg-muted">{initials}</span>
                  )}
                </div>
                <IconButton size="sm" variant="secondary" icon={ImagePlus} label="Change photo" loading={uploadingAvatar}
                  onClick={() => avatarInputRef.current?.click()} className="absolute -bottom-1.5 -right-1.5 shadow-card" />
                <input ref={avatarInputRef} type="file" accept="image/*" className="hidden" onChange={handleAvatarChange} />
              </div>

              <div className="min-w-0 space-y-1">
                <ResultNote result={avatarResult} inline />
                <p className="text-lg font-semibold text-fg">{profile?.full_name}</p>
                <p className="text-sm text-fg-muted">{profile?.email}</p>
                <div className="flex items-center gap-2">
                  <Pill tone="violet">{roleLabel}</Pill>
                  <span className="text-xs capitalize text-fg-muted">{profile?.portal_type ?? "internal"} portal</span>
                </div>
              </div>
            </div>
          </CardBody>
        </Card>

        <Card>
          <CardHeader title="Personal information" description="Full name, department and position are managed by HR. Ask your HR admin to change them." />
          <CardBody className="space-y-4">
            <FieldGrid cols={2}>
              <Field label="Full name"><ReadOnly value={profile?.full_name} /></Field>
              <Field label="Phone">
                <Input value={phone} onChange={(e) => setPhone(e.target.value)} placeholder="+1 ..." autoComplete="tel" />
              </Field>
              <Field label="Department"><ReadOnly value={profile?.department} /></Field>
              <Field label="Position"><ReadOnly value={profile?.position} /></Field>
            </FieldGrid>
            <ResultNote result={saveResult} />
          </CardBody>
          <CardFooter>
            <Button variant="primary" onClick={handleSaveProfile} loading={saving}>
              {!saving && <Save aria-hidden />} {saving ? "Saving..." : "Save phone"}
            </Button>
          </CardFooter>
        </Card>

        <Card>
          <CardHeader title="My signature" description="Draw or upload your signature once, then reuse it when signing documents."
            actions={savedSig ? <Pill tone="blue">Saved</Pill> : undefined} />
          <CardBody>
            <SignaturePad value={sig} onChange={setSig} />
          </CardBody>
          <CardFooter className="justify-start">
            <Button variant="primary" onClick={saveSignature} disabled={!sig} loading={savingSig}>
              {!savingSig && <Save aria-hidden />} {savingSig ? "Saving..." : "Save signature"}
            </Button>
            {savedSig && <Button variant="danger" onClick={removeSignature}><Trash2 aria-hidden /> Remove</Button>}
            <ResultNote result={sigResult} inline />
          </CardFooter>
        </Card>

        <Card>
          <CardHeader title="Change password" description="Changing it signs you out everywhere, this device included." />
          <CardBody className="space-y-4">
            <Field label="Current password">
              <Input type={showCurrent ? "text" : "password"} value={pwForm.current_password} autoComplete="current-password"
                onChange={(e) => setPwForm((f) => ({ ...f, current_password: e.target.value }))} className="pr-11" />
            </Field>
            <div className="-mt-2 flex justify-end">
              <Button size="sm" variant="ghost" onClick={() => setShowCurrent(!showCurrent)} aria-pressed={showCurrent}>
                {showCurrent ? <EyeOff aria-hidden /> : <Eye aria-hidden />} {showCurrent ? "Hide" : "Show"} current password
              </Button>
            </div>
            <FieldGrid cols={2}>
              <Field label="New password" hint="At least 8 characters.">
                <Input type={showNew ? "text" : "password"} value={pwForm.new_password} autoComplete="new-password"
                  onChange={(e) => setPwForm((f) => ({ ...f, new_password: e.target.value }))} />
              </Field>
              <Field label="Confirm new password" error={mismatch ? "Passwords do not match" : undefined}>
                <Input type="password" value={pwForm.confirm} autoComplete="new-password"
                  onChange={(e) => setPwForm((f) => ({ ...f, confirm: e.target.value }))} />
              </Field>
            </FieldGrid>
            <div className="-mt-2">
              <Button size="sm" variant="ghost" onClick={() => setShowNew(!showNew)} aria-pressed={showNew}>
                {showNew ? <EyeOff aria-hidden /> : <Eye aria-hidden />} {showNew ? "Hide" : "Show"} new password
              </Button>
            </div>
            <ResultNote result={pwResult} />
          </CardBody>
          <CardFooter>
            <Button variant="primary" onClick={handleChangePassword} loading={changingPw}>
              {!changingPw && <Lock aria-hidden />} {changingPw ? "Changing..." : "Change password"}
            </Button>
          </CardFooter>
        </Card>

        {/* Security: two-factor sign-in (TOTP, 2 Nov 2026) */}
        <section id="security" aria-label="Security">
          <TwoFactorSection />
        </section>

        <MyEmailSignature />
        <MyWhatsAppSignature />
        <MyWhatsAppChats />
      </div>
    </div>
  );
}
