/**
 * Unit tests for the remote-devices store (`$lib/stores/remoteDevices`).
 * The IPC client is fully mocked (`vi.mock`) — no Tauri runtime is touched.
 * Prompt-queue behavior mirrors the AuthStore suites in `ops.svelte.test`.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  deviceAdd,
  deviceAuthRespond,
  deviceConnect,
  deviceDisconnect,
  deviceForgetHostKey,
  deviceHostkeyRespond,
  deviceList,
  deviceRemove,
  deviceUpdate,
} from "$lib/ipc/client";
import { getToasts } from "$lib/toast";
import {
  PROMPT_TIMEOUT_MS,
  RemoteDeviceStore,
  deviceInputFromProfile,
} from "$lib/stores/remoteDevices.svelte";
import type { DeviceInfo, DeviceProfile } from "$lib/ipc/types";

vi.mock("$lib/ipc/client", () => ({
  deviceAdd: vi.fn(),
  deviceAuthRespond: vi.fn(),
  deviceConnect: vi.fn(),
  deviceDisconnect: vi.fn(),
  deviceForgetHostKey: vi.fn(),
  deviceHostkeyRespond: vi.fn(),
  deviceList: vi.fn(),
  deviceRemove: vi.fn(),
  deviceUpdate: vi.fn(),
}));

const mockDeviceList = vi.mocked(deviceList);
const mockDeviceAdd = vi.mocked(deviceAdd);
const mockDeviceUpdate = vi.mocked(deviceUpdate);
const mockDeviceRemove = vi.mocked(deviceRemove);
const mockDeviceConnect = vi.mocked(deviceConnect);
const mockDeviceDisconnect = vi.mocked(deviceDisconnect);
const mockDeviceForgetHostKey = vi.mocked(deviceForgetHostKey);
const mockHostkeyRespond = vi.mocked(deviceHostkeyRespond);
const mockAuthRespond = vi.mocked(deviceAuthRespond);

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

function profile(id: string, name = id): DeviceProfile {
  return {
    id,
    name,
    host: `${name}.example.com`,
    port: 22,
    username: "deploy",
    auth: { kind: "password", key_path: null, has_passphrase: false },
    poll_secs: 5,
  };
}

function device(id: string, status: DeviceInfo["status"] = "disconnected"): DeviceInfo {
  return { profile: profile(id), status };
}

function hostkeyRequest(
  requestId: string,
  deviceId: string,
  status: "unknown" | "changed" = "unknown",
) {
  return {
    request_id: requestId,
    device_id: deviceId,
    host: "server.example.com",
    port: 22,
    algo: "ssh-ed25519",
    fingerprint: "SHA256:abc",
    status,
    pinned_fingerprint: status === "changed" ? "SHA256:old" : null,
  };
}

function authRequest(requestId: string, deviceId: string) {
  return {
    request_id: requestId,
    device_id: deviceId,
    kind: "device-password" as const,
    prompt: "Password:",
  };
}

beforeEach(() => {
  mockDeviceList.mockReset();
  mockDeviceAdd.mockReset();
  mockDeviceUpdate.mockReset();
  mockDeviceRemove.mockReset();
  mockDeviceConnect.mockReset();
  mockDeviceDisconnect.mockReset();
  mockDeviceForgetHostKey.mockReset();
  mockHostkeyRespond.mockReset();
  mockAuthRespond.mockReset();
  mockHostkeyRespond.mockResolvedValue(undefined);
  mockAuthRespond.mockResolvedValue(undefined);
});

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

describe("registry (load / add / update / remove)", () => {
  it("load fills the device list", async () => {
    mockDeviceList.mockResolvedValue([device("d1"), device("d2", "connected")]);
    const store = new RemoteDeviceStore();

    await store.load();

    expect(mockDeviceList).toHaveBeenCalledOnce();
    expect(store.devices).toHaveLength(2);
    expect(store.statusOf("d2")).toBe("connected");
  });

  it("load keeps the current list on failure (never throws)", async () => {
    mockDeviceList.mockRejectedValue(new Error("backend gone") as never);
    const store = new RemoteDeviceStore();
    await store.load();
    expect(store.devices).toEqual([]);
  });

  it("add appends, update replaces, remove drops the entry", async () => {
    mockDeviceList.mockResolvedValue([device("d1")]);
    const store = new RemoteDeviceStore();
    await store.load();

    mockDeviceAdd.mockResolvedValue(device("d2"));
    const added = await store.add(deviceInputFromProfile(profile("d2")));
    expect(added.profile.id).toBe("d2");
    expect(store.devices.map((d) => d.profile.id)).toEqual(["d1", "d2"]);

    const updated = { ...device("d1", "connected") };
    mockDeviceUpdate.mockResolvedValue(updated);
    await store.update(deviceInputFromProfile(profile("d1")));
    expect(store.byId("d1")?.status).toBe("connected");

    mockDeviceRemove.mockResolvedValue(undefined);
    await store.remove("d1");
    expect(mockDeviceRemove).toHaveBeenCalledWith("d1");
    expect(store.devices.map((d) => d.profile.id)).toEqual(["d2"]);
  });

  it("connect marks connecting then connected; failures fall back + toast", async () => {
    mockDeviceList.mockResolvedValue([device("d1")]);
    const store = new RemoteDeviceStore();
    await store.load();

    mockDeviceConnect.mockResolvedValue("connected");
    const first = store.connect("d1");
    expect(store.statusOf("d1")).toBe("connecting");
    await first;
    expect(store.statusOf("d1")).toBe("connected");
    expect(getToasts()).toEqual([]);

    mockDeviceConnect.mockRejectedValue(new Error("no route") as never);
    const errorsBefore = getToasts().filter((t) => t.kind === "error").length;
    await store.connect("d1");
    expect(store.statusOf("d1")).toBe("disconnected");
    expect(getToasts().filter((t) => t.kind === "error").length).toBe(
      errorsBefore + 1,
    );
  });

  it("disconnect sets the status even when the command fails", async () => {
    mockDeviceList.mockResolvedValue([device("d1", "connected")]);
    const store = new RemoteDeviceStore();
    await store.load();

    mockDeviceDisconnect.mockRejectedValue(new Error("already gone") as never);
    await store.disconnect("d1");
    expect(store.statusOf("d1")).toBe("disconnected");
  });

  it("applyStatus patches only known devices", () => {
    const store = new RemoteDeviceStore();
    store.devices = [device("d1")];
    store.applyStatus({ device_id: "d1", status: "connected" });
    expect(store.statusOf("d1")).toBe("connected");
    store.applyStatus({ device_id: "ghost", status: "connected" });
    expect(store.byId("ghost")).toBeNull();
  });

  it("forgetHostKey resolves whether a key was pinned and never throws", async () => {
    const store = new RemoteDeviceStore();
    mockDeviceForgetHostKey.mockResolvedValue(true);
    await expect(store.forgetHostKey("d1")).resolves.toBe(true);
    mockDeviceForgetHostKey.mockRejectedValue(new Error("io") as never);
    await expect(store.forgetHostKey("d1")).resolves.toBe(false);
  });
});

// ---------------------------------------------------------------------------
// Pending host-key prompt queue
// ---------------------------------------------------------------------------

describe("host-key prompt queue", () => {
  it("shows the first request and queues the rest", () => {
    const store = new RemoteDeviceStore();
    store.applyHostKeyRequest(hostkeyRequest("r1", "d1"));
    store.applyHostKeyRequest(hostkeyRequest("r2", "d1"));

    expect(store.hostkeyPending?.request_id).toBe("r1");
  });

  it("answerHostKey responds and advances the queue", async () => {
    const store = new RemoteDeviceStore();
    store.applyHostKeyRequest(hostkeyRequest("r1", "d1"));
    store.applyHostKeyRequest(hostkeyRequest("r2", "d1"));

    await store.answerHostKey(true);
    expect(mockHostkeyRespond).toHaveBeenCalledWith("r1", true);
    expect(store.hostkeyPending?.request_id).toBe("r2");

    await store.dismissHostKey();
    expect(mockHostkeyRespond).toHaveBeenCalledWith("r2", false);
    expect(store.hostkeyPending).toBeNull();
  });

  it("a changed-key request surfaces the pinned vs new fingerprints", () => {
    const store = new RemoteDeviceStore();
    store.applyHostKeyRequest(hostkeyRequest("r1", "d1", "changed"));
    expect(store.hostkeyPending?.status).toBe("changed");
    expect(store.hostkeyPending?.pinned_fingerprint).toBe("SHA256:old");
  });

  it("removing a device drops its pending + queued prompts", async () => {
    mockDeviceList.mockResolvedValue([device("d1"), device("d2")]);
    mockDeviceRemove.mockResolvedValue(undefined);
    const store = new RemoteDeviceStore();
    await store.load();
    store.applyHostKeyRequest(hostkeyRequest("r1", "d1"));
    store.applyHostKeyRequest(hostkeyRequest("r2", "d2"));

    await store.remove("d1");
    expect(mockHostkeyRespond).not.toHaveBeenCalled();
    expect(store.hostkeyPending?.request_id).toBe("r2");
  });
});

// ---------------------------------------------------------------------------
// Pending device-auth prompt queue + stale pruning
// ---------------------------------------------------------------------------

describe("device-auth prompt queue", () => {
  it("answerAuth passes the password + store flag and advances", async () => {
    const store = new RemoteDeviceStore();
    store.applyAuthRequest(authRequest("a1", "d1"));
    store.applyAuthRequest(authRequest("a2", "d1"));

    await store.answerAuth("hunter2", true);
    expect(mockAuthRespond).toHaveBeenCalledWith("a1", "hunter2", true);
    expect(store.authPending?.request_id).toBe("a2");

    await store.dismissAuth();
    expect(mockAuthRespond).toHaveBeenCalledWith("a2", null, false);
    expect(store.authPending).toBeNull();
  });

  it("pruneStale drops prompts older than the backend timeout", () => {
    const store = new RemoteDeviceStore();
    store.applyHostKeyRequest(hostkeyRequest("r1", "d1"));
    store.applyAuthRequest(authRequest("a1", "d1"));

    // Fresh prompts survive a prune at "now".
    store.pruneStale(Date.now());
    expect(store.hostkeyPending?.request_id).toBe("r1");
    expect(store.authPending?.request_id).toBe("a1");

    // Past the 120 s backend timeout, the visible prompts are pruned (the
    // backend stopped waiting for their answers already).
    const later = Date.now() + PROMPT_TIMEOUT_MS + 1;
    store.pruneStale(later);
    expect(store.hostkeyPending).toBeNull();
    expect(store.authPending).toBeNull();

    // ... and the next request surfaces directly again (pruned by a fresh
    // "now", not the stale clock the old prompts died by).
    store.applyHostKeyRequest(hostkeyRequest("r2", "d1"));
    store.pruneStale(Date.now());
    expect(store.hostkeyPending?.request_id).toBe("r2");
  });
});
