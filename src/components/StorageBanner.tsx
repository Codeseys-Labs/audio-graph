/**
 * Top-of-window banner shown when the backend reports storage-full
 * (`CAPTURE_STORAGE_FULL`) on a transcript or graph write.
 *
 * Payloads reach this component via a module-level publisher —
 * `useTauriEvents` calls `publishStorageFull(payload)` when the backend
 * event fires, and all mounted `StorageBanner` instances (only one, at
 * the App root) receive it through a local listener set. This indirection
 * lets the hook emit into a React component without coupling either to
 * the store.
 *
 * The "Retry" button invokes `retry_storage_write` on the backend (see
 * `persistence::retry_storage_write`): on success the banner dismisses;
 * on failure the banner stays up with a "still full" hint so the user
 * knows they still need to free space.
 *
 * Parent: `App.tsx`. No props.
 */

import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import type { CaptureStorageFullPayload } from "../types";
import { errorToMessage } from "../utils/errorToMessage";
import Icon from "./Icon";
import IconButton from "./IconButton";

type Listener = (payload: CaptureStorageFullPayload) => void;

const listeners = new Set<Listener>();

export function publishStorageFull(payload: CaptureStorageFullPayload): void {
  for (const fn of listeners) fn(payload);
}

function StorageBanner() {
  const { t } = useTranslation();
  const [current, setCurrent] = useState<CaptureStorageFullPayload | null>(
    null,
  );
  const [retrying, setRetrying] = useState(false);
  const [retryError, setRetryError] = useState<string | null>(null);

  useEffect(() => {
    const listener: Listener = (payload) => {
      // Fresh banner → clear any stale "still full" message from a
      // previous attempt.
      setRetryError(null);
      setCurrent(payload);
    };
    listeners.add(listener);
    return () => {
      listeners.delete(listener);
    };
  }, []);

  if (!current) return null;

  const handleResume = async () => {
    setRetrying(true);
    setRetryError(null);
    try {
      await invoke("retry_storage_write");
      console.info("StorageBanner: user acknowledged storage-full, resuming");
      setCurrent(null);
    } catch (e) {
      // Probe failed — disk is still full. Keep the banner up and show
      // the backend's error so the user knows to free more space.
      const msg = errorToMessage(e);
      console.warn("StorageBanner: retry failed:", msg);
      setRetryError(msg);
    } finally {
      setRetrying(false);
    }
  };
  const handleDismiss = () => {
    setRetryError(null);
    setCurrent(null);
  };

  return (
    <div
      className="banner-on-accent flex items-center gap-(--space-5) py-[10px] px-(--space-6) bg-[#c9402f] text-white text-md shadow-1 z-[1100]"
      role="alert"
      aria-live="assertive"
      data-testid="storage-banner"
    >
      <span className="text-xl shrink-0" aria-hidden="true">
        <Icon name="warning" />
      </span>
      <div className="flex flex-col flex-1 gap-(--space-1) leading-[1.3]">
        <strong className="font-semibold">{t("storage.title")}</strong>
        <span className="opacity-95">{t("storage.message")}</span>
        {retryError !== null && (
          <span
            className="storage-banner__error"
            data-testid="storage-banner-error"
            role="status"
          >
            {retryError}
          </span>
        )}
      </div>
      <button
        type="button"
        className="bg-[rgba(255,255,255,0.18)] border border-[rgba(255,255,255,0.45)] text-white cursor-pointer text-md py-[5px] px-(--space-5) rounded-sm shrink-0 transition-colors hover:bg-[rgba(255,255,255,0.3)]"
        onClick={handleResume}
        disabled={retrying}
      >
        {t("storage.resume")}
      </button>
      <IconButton
        icon="close"
        label={t("storage.dismiss")}
        variant="ghost"
        className="bg-none border-none text-[rgba(255,255,255,0.95)] cursor-pointer text-lg py-(--space-1) px-(--space-3) rounded-sm shrink-0 leading-none hover:text-white hover:bg-[rgba(255,255,255,0.15)]"
        onClick={handleDismiss}
      />
    </div>
  );
}

export default StorageBanner;
