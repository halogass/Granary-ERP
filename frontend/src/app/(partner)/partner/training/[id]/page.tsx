"use client";

import { useState, useEffect } from "react";
import { useSession } from "next-auth/react";
import { useParams, useRouter } from "next/navigation";
import Link from "next/link";
import { ArrowLeft, ArrowRight, Award, Check, CircleHelp, Download, RotateCcw } from "lucide-react";
import { Button, Card, CardBody, EmptyState, Skeleton, toast } from "@/components/ui";
import { cn } from "@/lib/utils";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

// Markdown chapter text (no typography plugin in this app).
const MD = "text-sm leading-relaxed text-fg [&_a]:text-brand-ink [&_a]:underline [&_blockquote]:border-l-2 [&_blockquote]:border-line " +
  "[&_blockquote]:pl-3 [&_blockquote]:text-fg-muted [&_code]:rounded [&_code]:bg-surface-2 [&_code]:px-1 [&_code]:font-mono [&_code]:text-[13px] " +
  "[&_h1]:mb-2 [&_h1]:mt-5 [&_h1]:text-lg [&_h1]:font-semibold [&_h2]:mb-2 [&_h2]:mt-5 [&_h2]:text-base [&_h2]:font-semibold " +
  "[&_h3]:mb-1 [&_h3]:mt-4 [&_h3]:font-semibold [&_li]:my-0.5 [&_ol]:list-decimal [&_ol]:pl-5 [&_p]:my-2 [&_pre]:overflow-x-auto " +
  "[&_pre]:rounded-lg [&_pre]:bg-surface-2 [&_pre]:p-3 [&_table]:w-full [&_td]:border [&_td]:border-line [&_td]:px-2 [&_td]:py-1 " +
  "[&_th]:border [&_th]:border-line [&_th]:bg-surface-2 [&_th]:px-2 [&_th]:py-1 [&_th]:text-left [&_ul]:list-disc [&_ul]:pl-5";

interface QuizQuestion {
  id: string;
  question: string;
  options: string[];
  order: number;
}

interface Chapter {
  id: string;
  title: string;
  content: string;
  video_url: string | null;
  order: number;
  questions: QuizQuestion[];
}

interface Module {
  id: string;
  title: string;
  description: string;
  pass_score: number;
  chapter_count: number;
  chapters: Chapter[];
  progress?: {
    status: string;
    current_chapter: number;
    score: number | null;
    completed_at: string | null;
  };
}

export default function TrainingModulePage() {
  const { data: session } = useSession();
  const params = useParams();
  const router = useRouter();
  const moduleId = params.id as string;

  const [mod, setMod] = useState<Module | null>(null);
  const [loading, setLoading] = useState(true);
  const [activeChapterIdx, setActiveChapterIdx] = useState(0);
  const [readChapters, setReadChapters] = useState<Set<string>>(new Set());
  const [showQuiz, setShowQuiz] = useState(false);
  const [answers, setAnswers] = useState<Record<string, number>>({});
  // Before a pass the server gives only the verdict and the attempts left (no score: it would give the answers away).
  const [quizResult, setQuizResult] = useState<{ score?: number; passed: boolean; attempts_left?: number; cert_url?: string } | null>(null);
  const [submitting, setSubmitting] = useState(false);

  const token = session?.access_token;

  useEffect(() => {
    if (!token) return;
    fetch(`/api/v1/partner/portal/training/${moduleId}`, { headers: { Authorization: `Bearer ${token}` } })
      .then(r => r.json())
      .then(data => {
        setMod(data);
        if (data.progress?.current_chapter > 0) {
          setActiveChapterIdx(Math.min(data.progress.current_chapter, (data.chapters?.length ?? 1) - 1));
          const read = new Set<string>();
          for (let i = 0; i < data.progress.current_chapter; i++) {
            if (data.chapters?.[i]) read.add(data.chapters[i].id);
          }
          setReadChapters(read);
        }
      })
      .catch(() => toast.error("Failed to load module"))
      .finally(() => setLoading(false));

    // Start tracking
    fetch(`/api/v1/partner/portal/training/${moduleId}/start`, { method: "POST", headers: { Authorization: `Bearer ${token}` } }).catch(() => {});
  }, [token, moduleId]);

  function markRead(chapterId: string) {
    setReadChapters(prev => new Set([...prev, chapterId]));
  }

  const allChaptersRead = mod ? mod.chapters?.every(c => readChapters.has(c.id)) : false;
  const allQuestions = mod?.chapters?.flatMap(c => c.questions) ?? [];
  const hasQuiz = allQuestions.length > 0;

  async function submitQuiz() {
    if (!token) return;
    const ansArr = allQuestions.map(q => ({ question_id: q.id, selected_index: answers[q.id] ?? -1 }));
    setSubmitting(true);
    try {
      const res = await fetch(`/api/v1/partner/portal/training/${moduleId}/submit-quiz`, {
        method: "POST",
        headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
        body: JSON.stringify({ answers: ansArr }),
      });
      if (!res.ok) {
        const d = await res.json().catch(() => ({}));
        toast.error(typeof d.detail === "string" ? d.detail : "Failed to submit quiz");
        return;
      }
      const result = await res.json();
      setQuizResult(result);
      if (mod) {
        setMod(prev => prev ? { ...prev, progress: { ...prev.progress, status: result.passed ? "passed" : "failed", score: result.score ?? null, current_chapter: prev.chapters?.length ?? 0, completed_at: new Date().toISOString() } } : null);
      }
    } catch { toast.error("Failed to submit quiz"); }
    finally { setSubmitting(false); }
  }

  // cert_url is an API path that needs the partner's token: fetch it and save the PDF (a plain link would 401)
  async function downloadCertificate(url: string) {
    if (!token) return;
    try {
      const res = await fetch(url, { headers: { Authorization: `Bearer ${token}` } });
      if (!res.ok) throw new Error();
      const name = /filename="([^"]+)"/.exec(res.headers.get("content-disposition") ?? "")?.[1] ?? "certificate.pdf";
      const href = URL.createObjectURL(await res.blob());
      const a = document.createElement("a");
      a.href = href;
      a.download = name;
      a.click();
      URL.revokeObjectURL(href);
    } catch { toast.error("Could not download the certificate"); }
  }

  if (loading) return (
    <div className="grid gap-4 grid-cols-1 lg:grid-cols-[16rem_minmax(0,1fr)]" aria-busy="true" aria-label="Loading module">
      <Skeleton className="h-72" /><Skeleton className="h-96" />
    </div>
  );
  if (!mod) return (
    <Card><EmptyState title="Module not found" description="It may have been unpublished."
      action={<Button asChild><Link href="/partner/training">Back to training</Link></Button>} /></Card>
  );

  const chapters = mod.chapters ?? [];
  const activeChapter = chapters[activeChapterIdx];

  const isPassed = mod.progress?.status === "passed" || quizResult?.passed;
  const answered = Object.keys(answers).length;
  const navItem = (active: boolean) => cn("flex min-h-11 w-full items-start gap-2.5 rounded-lg px-2.5 py-2 text-left transition-colors md:min-h-0",
    active ? "bg-brand/[0.07] text-fg shadow-[inset_2px_0_0_rgb(var(--brand))]" : "text-fg-muted hover:bg-surface-2 hover:text-fg");

  return (
    <div className="grid items-start gap-4 grid-cols-1 lg:grid-cols-[16rem_minmax(0,1fr)]">
      <Card className="lg:sticky lg:top-32">
        <div className="border-b border-line p-4">
          <Link href="/partner/training" className="mb-2 inline-flex items-center gap-1 text-xs text-fg-muted hover:text-fg">
            <ArrowLeft className="size-3.5" aria-hidden /> Training
          </Link>
          <h1 className="text-sm font-semibold leading-snug text-fg">{mod.title}</h1>
          <p className="mt-1 text-xs text-fg-muted">Pass score: {mod.pass_score}%</p>
        </div>
        <nav aria-label="Chapters" className="space-y-0.5 p-2">
          {chapters.map((ch, i) => {
            const isRead = readChapters.has(ch.id);
            const isActive = i === activeChapterIdx && !showQuiz && !quizResult;
            return (
              <button key={ch.id} type="button" aria-current={isActive ? "step" : undefined}
                onClick={() => { setActiveChapterIdx(i); setShowQuiz(false); }} className={navItem(isActive)}>
                <span className={cn("mt-px flex size-5 shrink-0 items-center justify-center rounded-full border text-[10px] font-semibold tabular",
                  isRead ? "border-tone-blue bg-tone-blue text-surface" : isActive ? "border-brand text-brand-ink" : "border-line-strong text-fg-subtle")}>
                  {isRead ? <Check className="size-3" aria-label="Read" /> : i + 1}
                </span>
                <span className="text-[13px] font-medium leading-snug">{ch.title}</span>
              </button>
            );
          })}
          {hasQuiz && (
            <button type="button" onClick={() => { if (allChaptersRead) setShowQuiz(true); }} disabled={!allChaptersRead}
              title={allChaptersRead ? undefined : "Read every chapter first"}
              className={cn(navItem(showQuiz && !!allChaptersRead), "mt-1", !allChaptersRead && "cursor-not-allowed opacity-50")}>
              <CircleHelp className="mt-px size-5 shrink-0" aria-hidden />
              <span className="text-[13px] font-medium">Final quiz</span>
            </button>
          )}
        </nav>

        <div className="border-t border-line p-4">
          <div className="mb-1.5 flex justify-between text-xs text-fg-muted">
            <span>Progress</span>
            <span className="tabular">{readChapters.size}/{chapters.length}</span>
          </div>
          <div className="h-1.5 overflow-hidden rounded-full bg-surface-2">
            <div className="h-full rounded-full bg-brand transition-all" style={{ width: `${chapters.length > 0 ? (readChapters.size / chapters.length) * 100 : 0}%` }} />
          </div>
          {mod.progress?.score != null && (
            <p className="mt-2 text-xs text-fg-muted">Last score: <span className={cn("font-semibold tabular", isPassed ? "text-tone-blue" : "text-tone-red")}>{mod.progress.score}%</span></p>
          )}
        </div>
      </Card>

      <Card className="min-w-0">
        {quizResult ? (
          <CardBody className="mx-auto max-w-lg py-12 text-center">
            <span className={cn("mx-auto mb-5 flex size-16 items-center justify-center rounded-full",
              quizResult.passed ? "bg-tone-blue-soft text-tone-blue" : "bg-tone-red-soft text-tone-red")}>
              {quizResult.passed ? <Award className="size-8" aria-hidden /> : <RotateCcw className="size-8" aria-hidden />}
            </span>
            <h2 className="text-lg font-semibold text-fg">{quizResult.passed ? "Congratulations, you passed" : "Not quite there yet"}</h2>
            <p className="mt-2 text-sm text-fg-muted">
              {quizResult.passed && quizResult.score != null ? (
                <>You scored <span className="text-base font-semibold tabular text-tone-blue">{quizResult.score}%</span>{" "}(pass: {mod.pass_score}%)</>
              ) : (
                <>You did not reach the pass mark of {mod.pass_score}%.
                  {quizResult.attempts_left != null && ` Attempts left: ${quizResult.attempts_left}.`}</>
              )}
            </p>
            {quizResult.passed && (
              <div className="mt-6 rounded-lg border border-tone-blue/30 bg-tone-blue-soft p-4">
                <p className="text-sm font-semibold text-tone-blue">Your certification has been issued.</p>
                {quizResult.cert_url && (
                  <Button variant="primary" className="mt-3" onClick={() => downloadCertificate(quizResult.cert_url!)}>
                    <Download aria-hidden /> Download certificate
                  </Button>
                )}
              </div>
            )}
            <div className="mt-6 flex flex-wrap justify-center gap-2">
              <Button onClick={() => { setShowQuiz(false); setQuizResult(null); setAnswers({}); }}>
                {quizResult.passed ? "Review content" : "Try again"}
              </Button>
              <Button variant="primary" onClick={() => router.push("/partner/training")}>Back to training</Button>
            </div>
          </CardBody>
        ) : showQuiz ? (
          <CardBody className="mx-auto max-w-2xl md:p-8">
            <h2 className="text-lg font-semibold text-fg">Final quiz</h2>
            <p className="mb-6 mt-1 text-sm text-fg-muted">{allQuestions.length} questions, {mod.pass_score}% needed to pass</p>
            <ol className="space-y-4">
              {allQuestions.map((q, qi) => (
                <li key={q.id} className="rounded-lg border border-line p-4">
                  <fieldset>
                    <legend className="mb-3 text-sm font-semibold text-fg">{qi + 1}. {q.question}</legend>
                    <div className="space-y-2">
                      {q.options.map((opt, oi) => {
                        const on = answers[q.id] === oi;
                        return (
                          <button key={oi} type="button" aria-pressed={on} onClick={() => setAnswers(prev => ({ ...prev, [q.id]: oi }))}
                            className={cn("flex min-h-11 w-full items-center rounded-lg border px-3 py-2 text-left text-sm transition-colors md:min-h-9",
                              on ? "border-brand bg-brand/[0.07] font-medium text-fg" : "border-line text-fg hover:bg-surface-2")}>
                            {opt}
                          </button>
                        );
                      })}
                    </div>
                  </fieldset>
                </li>
              ))}
            </ol>
            <div className="mt-6 flex flex-wrap gap-2">
              <Button onClick={() => setShowQuiz(false)}>Back to content</Button>
              <Button variant="primary" className="flex-1" onClick={submitQuiz} loading={submitting} disabled={answered < allQuestions.length}>
                {submitting ? "Submitting..." : `Submit quiz (${answered}/${allQuestions.length} answered)`}
              </Button>
            </div>
          </CardBody>
        ) : activeChapter ? (
          <CardBody className="mx-auto max-w-2xl md:p-8">
            <div className="mb-6 flex flex-wrap items-start justify-between gap-3">
              <div>
                <p className="mb-1 text-xs text-fg-muted tabular">Chapter {activeChapterIdx + 1} of {chapters.length}</p>
                <h2 className="text-lg font-semibold text-fg">{activeChapter.title}</h2>
              </div>
              {!readChapters.has(activeChapter.id) && (
                <Button size="sm" onClick={() => markRead(activeChapter.id)}><Check aria-hidden /> Mark read</Button>
              )}
            </div>

            {activeChapter.video_url && (
              <div className="mb-6 aspect-video overflow-hidden rounded-lg bg-black">
                <iframe
                  src={activeChapter.video_url.replace("watch?v=", "embed/")}
                  title={activeChapter.title}
                  className="size-full"
                  allow="accelerometer; autoplay; clipboard-write; encrypted-media; gyroscope; picture-in-picture"
                  allowFullScreen
                />
              </div>
            )}

            <div className={MD}>
              <ReactMarkdown remarkPlugins={[remarkGfm]}>{activeChapter.content}</ReactMarkdown>
            </div>

            <div className="mt-10 flex items-center justify-between gap-2 border-t border-line pt-5">
              <Button onClick={() => setActiveChapterIdx(i => Math.max(0, i - 1))} disabled={activeChapterIdx === 0}>
                <ArrowLeft aria-hidden /> Previous
              </Button>
              {activeChapterIdx < chapters.length - 1 ? (
                <Button variant="primary" onClick={() => { markRead(activeChapter.id); setActiveChapterIdx(i => i + 1); }}>
                  Next <ArrowRight aria-hidden />
                </Button>
              ) : (
                hasQuiz && allChaptersRead ? (
                  <Button variant="primary" onClick={() => { markRead(activeChapter.id); setShowQuiz(true); }}>
                    Take quiz <ArrowRight aria-hidden />
                  </Button>
                ) : (
                  <Button variant="primary" onClick={() => markRead(activeChapter.id)}>
                    <Check aria-hidden /> {readChapters.has(activeChapter.id) ? "Completed" : "Mark complete"}
                  </Button>
                )
              )}
            </div>
          </CardBody>
        ) : (
          <EmptyState title="Select a chapter to begin" />
        )}
      </Card>
    </div>
  );
}
