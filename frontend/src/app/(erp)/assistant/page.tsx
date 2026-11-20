"use client";

import { Suspense, useEffect } from "react";
import { useRouter, useSearchParams } from "next/navigation";

/** The Granary Assistant is now Nary's chat; old links (and their ?c= conversation) land there. */
function Redirect() {
  const router = useRouter();
  const params = useSearchParams();
  useEffect(() => {
    const c = params.get("c");
    router.replace(c ? `/nary/chat?c=${encodeURIComponent(c)}` : "/nary/chat");
  }, [router, params]);
  return null;
}

export default function AssistantRedirect() {
  return <Suspense fallback={null}><Redirect /></Suspense>;
}
