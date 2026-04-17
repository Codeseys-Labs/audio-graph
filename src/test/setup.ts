import "@testing-library/jest-dom";
import { vi } from "vitest";

// Mock the Tauri API so tests don't need a running Tauri runtime.
vi.mock("@tauri-apps/api/core", () => ({
    invoke: vi.fn(),
}));
vi.mock("@tauri-apps/api/event", () => ({
    listen: vi.fn(async () => () => {}),
    emit: vi.fn(),
}));
