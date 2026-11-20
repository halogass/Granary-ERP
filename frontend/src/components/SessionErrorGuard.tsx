"use client";

import { useSession, signOut } from "next-auth/react";
import { useEffect } from "react";

export default function SessionErrorGuard() {
  const { data: session } = useSession();

  useEffect(() => {
    if (session?.error === "RefreshTokenExpired") {
      signOut({ callbackUrl: "/login" });
    }
  }, [session]);

  return null;
}
