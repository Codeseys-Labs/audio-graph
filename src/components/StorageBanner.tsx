import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import type { CaptureStorageFullPayload } from "../types";

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
            console.info(
                "StorageBanner: user acknowledged storage-full, resuming",
            );
            setCurrent(null);
        } catch (e) {
            // Probe failed — disk is still full. Keep the banner up and show
            // the backend's error so the user knows to free more space.
            const msg = e instanceof Error ? e.message : String(e);
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
            className="storage-banner"
            role="alert"
            aria-live="assertive"
            data-testid="storage-banner"
        >
            <span className="storage-banner__icon" aria-hidden="true">
                ⚠
            </span>
            <div className="storage-banner__body">
                <strong className="storage-banner__title">
                    {t("storage.title")}
                </strong>
                <span className="storage-banner__message">
                    {t("storage.message")}
                </span>
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
                className="storage-banner__resume"
                onClick={handleResume}
                disabled={retrying}
            >
                {t("storage.resume")}
            </button>
            <button
                type="button"
                className="storage-banner__dismiss"
                onClick={handleDismiss}
                aria-label={t("storage.dismiss")}
            >
                ✕
            </button>
        </div>
    );
}

export default StorageBanner;
