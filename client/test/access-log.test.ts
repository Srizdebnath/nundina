import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { logAccess, warnIfOpenInProduction } from "@/lib/access-log";

describe("access log (#202)", () => {
  afterEach(() => {
    delete process.env.NUNDINA_ACCESS_LOG;
    vi.unstubAllEnvs();
    vi.restoreAllMocks();
  });

  it("appends one JSON line per event to NUNDINA_ACCESS_LOG", () => {
    const file = join(mkdtempSync(join(tmpdir(), "acc-")), "access.jsonl");
    process.env.NUNDINA_ACCESS_LOG = file;
    logAccess("sign-in", "6vt1wHVQj7pXyvR5nYjWkSoLfQvR3DYbDhjFgYz1VKL4", "/api/access", "credit-gate validate approves");
    logAccess("gated-refused", null, "/data-room");
    const lines = readFileSync(file, "utf8").trim().split("\n").map((l) => JSON.parse(l));
    expect(lines).toHaveLength(2);
    expect(lines[0]).toMatchObject({ event: "sign-in", path: "/api/access", detail: "credit-gate validate approves", mode: "open" });
    expect(lines[1]).toMatchObject({ event: "gated-refused", wallet: null, path: "/data-room" });
    expect(Date.parse(lines[0].at)).not.toBeNaN();
  });

  it("goes to stdout without a file, and a write failure never throws", () => {
    const log = vi.spyOn(console, "log").mockImplementation(() => {});
    logAccess("challenge", "x", "/api/access");
    expect(log.mock.calls[0]![0]).toMatch(/^\[nundina-access\] \{"at"/);
    const err = vi.spyOn(console, "error").mockImplementation(() => {});
    process.env.NUNDINA_ACCESS_LOG = "/nonexistent-dir/x/access.jsonl";
    expect(() => logAccess("sign-out", null, "/api/access")).not.toThrow();
    expect(err).toHaveBeenCalled();
  });

  it("warns once when a production build runs in open mode", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    vi.stubEnv("NODE_ENV", "production");
    expect(warnIfOpenInProduction()).toBe(true);
    expect(warnIfOpenInProduction()).toBe(false);
    expect(warn).toHaveBeenCalledTimes(1);
    expect(warn.mock.calls[0]![0]).toMatch(/NUNDINA_ACCESS is not 'allowlist'/);
  });
});
