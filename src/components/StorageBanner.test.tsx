import {
    describe,
    it,
    expect,
    beforeEach,
    afterEach,
    vi,
} from "vitest";
import {
    render,
    screen,
    act,
    fireEvent,
    waitFor,
} from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import StorageBanner, { publishStorageFull } from "./StorageBanner";
import "../i18n";

const mockedInvoke = vi.mocked(invoke);

describe("StorageBanner", () => {
    let infoSpy: ReturnType<typeof vi.spyOn>;
    let warnSpy: ReturnType<typeof vi.spyOn>;
    beforeEach(() => {
        mockedInvoke.mockReset();
        infoSpy = vi.spyOn(console, "info").mockImplementation(() => {});
        warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
    });
    afterEach(() => {
        infoSpy.mockRestore();
        warnSpy.mockRestore();
    });

    it("renders nothing until a storage-full event is published", () => {
        render(<StorageBanner />);
        expect(
            screen.queryByTestId("storage-banner"),
        ).not.toBeInTheDocument();
    });

    it("appears with localized title + resume action on storage-full publish", () => {
        render(<StorageBanner />);

        act(() => {
            publishStorageFull({
                path: "/tmp/session/transcript.jsonl",
                bytes_written: 0,
                bytes_lost: 4096,
            });
        });

        const banner = screen.getByTestId("storage-banner");
        expect(banner).toBeInTheDocument();
        expect(banner).toHaveAttribute("role", "alert");
        expect(
            screen.getByRole("button", { name: /resume/i }),
        ).toBeInTheDocument();
        // Message text comes from the en.json storage.message key.
        expect(
            screen.getByText(/capture paused/i),
        ).toBeInTheDocument();
    });

    it("hides when the dismiss (✕) button is clicked", () => {
        render(<StorageBanner />);
        act(() => {
            publishStorageFull({
                path: "/tmp/x",
                bytes_written: 0,
                bytes_lost: 1024,
            });
        });
        expect(screen.getByTestId("storage-banner")).toBeInTheDocument();

        act(() => {
            fireEvent.click(
                screen.getByRole("button", { name: /dismiss/i }),
            );
        });
        expect(
            screen.queryByTestId("storage-banner"),
        ).not.toBeInTheDocument();
    });

    it("invokes retry_storage_write and dismisses the banner on success", async () => {
        mockedInvoke.mockResolvedValueOnce(undefined);
        render(<StorageBanner />);
        act(() => {
            publishStorageFull({
                path: "/tmp/x",
                bytes_written: 0,
                bytes_lost: 1024,
            });
        });

        await act(async () => {
            fireEvent.click(screen.getByRole("button", { name: /resume/i }));
        });

        expect(mockedInvoke).toHaveBeenCalledWith("retry_storage_write");
        await waitFor(() =>
            expect(
                screen.queryByTestId("storage-banner"),
            ).not.toBeInTheDocument(),
        );
        expect(infoSpy).toHaveBeenCalledWith(
            expect.stringContaining("acknowledged"),
        );
    });

    it("keeps the banner visible and surfaces the backend error when retry fails", async () => {
        mockedInvoke.mockRejectedValueOnce(
            "Storage still unavailable: No space left on device (os error 28)",
        );
        render(<StorageBanner />);
        act(() => {
            publishStorageFull({
                path: "/tmp/x",
                bytes_written: 0,
                bytes_lost: 1024,
            });
        });

        await act(async () => {
            fireEvent.click(screen.getByRole("button", { name: /resume/i }));
        });

        expect(mockedInvoke).toHaveBeenCalledWith("retry_storage_write");
        // Banner must stay up so the user can try again after freeing space.
        expect(screen.getByTestId("storage-banner")).toBeInTheDocument();

        // The backend's error message must be visible to the user.
        const errNode = await screen.findByTestId("storage-banner-error");
        expect(errNode).toHaveTextContent(/still unavailable/i);
        expect(warnSpy).toHaveBeenCalledWith(
            expect.stringContaining("retry failed"),
            expect.any(String),
        );
    });
});
