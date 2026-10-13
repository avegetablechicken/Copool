const { test } = require("node:test");
const assert = require("node:assert/strict");
const { Proxy } = require("../ui/controllers.js");
const P = require("../ui/presentation.js");
const legacy = (suite, name, fn) => test(`${suite}.${name}`, fn);
const snapshot = () => ({
  syncedAt: 1000,
  proxyStatus: { running: true, port: 8787 },
  cloudflaredStatus: {
    installed: true,
    running: true,
    publicURL: "https://sample.test/v1",
  },
  publicAccessEnabled: true,
  preferredProxyPortText: "8787",
  cloudflaredTunnelMode: "quick",
  cloudflaredUseHTTP2: false,
  cloudflaredNamedInput: {
    apiToken: "",
    accountID: "",
    zoneID: "",
    hostname: "",
  },
  remoteServers: [{ id: "server-1", label: "Tokyo" }],
  remoteStatuses: { "server-1": { running: true } },
  remoteLogs: {},
  remoteDiscoveries: {},
});
legacy(
  "ProxyPageModelTests",
  "testMacOSModelAppliesLocalSnapshotBroadcastWithoutPageReload",
  () => {
    const model = new Proxy();
    const running = snapshot();
    assert.equal(model.applySnapshot(running), true);
    assert.equal(model.tunnelConfiguration.enabled, true);
    assert.equal(model.tunnelStatus.running, true);
    const stopped = structuredClone(running);
    stopped.syncedAt += 1000;
    stopped.cloudflaredStatus.running = false;
    stopped.cloudflaredStatus.publicURL = null;
    stopped.publicAccessEnabled = false;
    model.applySnapshot(stopped);
    assert.equal(model.tunnelConfiguration.enabled, false);
    assert.equal(model.tunnelStatus.running, false);
  },
);
legacy(
  "ProxyPageModelTests",
  "testRefreshForTabEntryPreservesPublicAccessConfiguration",
  async () => {
    const model = new Proxy(async (command) => {
      if (command === "proxy_status") return { running: false };
      if (command === "configuration")
        return {
          remoteServers: [],
          proxyConfiguration: {
            preferredPortText: "8787",
            cloudflared: { enabled: true },
          },
        };
      if (command === "tunnel_action")
        return { running: false, installed: false };
      throw Error(command);
    });
    await model.refresh();
    assert.equal(model.tunnelConfiguration.enabled, true);
    assert.equal(model.tunnelStatus.running, false);
  },
);
legacy(
  "ProxyPageModelTests",
  "testApplyRemoteSnapshotPreservesLocalNamedTunnelSecrets",
  () => {
    const model = new Proxy();
    Object.assign(model.tunnelConfiguration, {
      apiToken: "secret-token",
      accountId: "account-secret",
      zoneId: "zone-secret",
      namedHostname: "local.example.com",
    });
    const s = snapshot();
    s.cloudflaredTunnelMode = "named";
    s.cloudflaredNamedInput.hostname = "synced.example.com";
    model.applySnapshot(s);
    assert.equal(model.tunnelConfiguration.apiToken, "secret-token");
    assert.equal(model.tunnelConfiguration.accountId, "account-secret");
    assert.equal(model.tunnelConfiguration.zoneId, "zone-secret");
    assert.equal(model.tunnelConfiguration.namedHostname, "synced.example.com");
  },
);
legacy(
  "ProxyPageModelTests",
  "testApplyRemoteSnapshotTrimsOversizedRemoteLogsToPolicyCap",
  () => {
    const model = new Proxy(),
      s = snapshot();
    s.remoteLogs["server-1"] = "0123456789".repeat(1400);
    model.applySnapshot(s);
    const expected = "...\n" + s.remoteLogs["server-1"].slice(-11996);
    assert.equal(model.remoteLogs["server-1"], expected);
    assert.equal(model.lastAppliedSnapshot.remoteLogs["server-1"], expected);
    assert.equal(expected.length, 12000);
  },
);
legacy(
  "ProxyPageModelTests",
  "testApplyRemoteSnapshotUpdatesSyncedConfigurationForMetadataOnlyAck",
  () => {
    const model = new Proxy(),
      s = snapshot();
    s.preferredProxyPortText = "9000";
    s.publicAccessEnabled = true;
    s.cloudflaredTunnelMode = "named";
    s.cloudflaredUseHTTP2 = true;
    s.cloudflaredNamedInput.hostname = "edge.example.com";
    assert.equal(model.applySnapshot(s), true);
    const ack = { ...s, syncedAt: 2000, lastHandledCommandID: "command-1" };
    assert.equal(model.applySnapshot(ack), false);
    assert.deepEqual(model.lastSyncedConfiguration, {
      preferredPortText: "9000",
      cloudflared: {
        enabled: true,
        tunnelMode: "named",
        useHTTP2: true,
        namedHostname: "edge.example.com",
      },
    });
  },
);
legacy(
  "ProxyPageModelTests",
  "testUpdateCloudflaredUseHTTP2SyncsProxyConfigurationLocally",
  async () => {
    const calls = [];
    const model = new Proxy(async (c, a) => calls.push([c, a]));
    await model.updateTunnel({ useHTTP2: true });
    assert.deepEqual(
      calls.map((c) => c[0]),
      ["save_tunnel_configuration"],
    );
    assert.equal(calls[0][1].value.useHTTP2, true);
  },
);
legacy(
  "ProxyPageModelTests",
  "testSetPublicAccessEnabledSyncsProxyConfigurationWithoutStartingCloudflared",
  async () => {
    const calls = [];
    const model = new Proxy(async (c, a) => calls.push([c, a]));
    const s = snapshot();
    s.cloudflaredStatus.running = false;
    s.publicAccessEnabled = false;
    model.applySnapshot(s);
    await model.setPublicAccessEnabled(true);
    assert.deepEqual(
      calls.map((c) => c[0]),
      ["save_tunnel_configuration"],
    );
    assert.equal(calls[0][1].value.enabled, true);
  },
);
legacy(
  "ProxyPageModelTests",
  "testPublicAccessEnabledBindingDispatchesConfigurationSync",
  async () => {
    const calls = [];
    const model = new Proxy(async (c, a) => calls.push([c, a]));
    await model.updateTunnel({ enabled: true });
    assert.equal(calls[0][0], "save_tunnel_configuration");
    assert.equal(calls[0][1].value.enabled, true);
  },
);
legacy(
  "ProxyPageModelTests",
  "testRemoteServerCardActionsUseInjectedIdentityPicker",
  async () => {
    const model = new Proxy(null, () => {}, {
      identityPicker: () => "/tmp/id_ed25519",
    });
    assert.equal(await model.chooseIdentityFile(), "/tmp/id_ed25519");
  },
);
legacy(
  "ProxyPageModelTests",
  "testCannotStartCloudflaredWhenPublicAccessDisabled",
  async () => {
    const calls = [];
    const model = new Proxy(async (c) => calls.push(c));
    const s = snapshot();
    s.publicAccessEnabled = false;
    s.cloudflaredStatus.running = false;
    model.applySnapshot(s);
    assert.equal(model.canStartTunnel, false);
    await assert.rejects(model.tunnelAction("start"));
    assert.equal(calls.length, 0);
  },
);
legacy(
  "ProxyPageModelTests",
  "testAPIProxyActionButtonsSwitchPrimaryIntentWithRuntimeState",
  () => {
    const stopped = P.proxyButtons(false, false),
      running = P.proxyButtons(true, false);
    assert.deepEqual(
      stopped.map((b) => b.intent),
      ["refreshStatus", "start"],
    );
    assert.equal(stopped[1].surfaceStyle, "prominent");
    assert.deepEqual(
      running.map((b) => b.intent),
      ["refreshStatus", "stop"],
    );
    assert.equal(running[1].role, "destructive");
  },
);
legacy(
  "ProxyPageModelTests",
  "testPublicAccessActionButtonsDisableStartWhenStartPreconditionsFail",
  () => {
    const buttons = P.publicButtons(false, false, false);
    assert.deepEqual(
      buttons.map((b) => b.intent),
      ["refreshStatus", "start"],
    );
    assert.equal(buttons[1].enabled, false);
  },
);
legacy(
  "ProxyPageModelTests",
  "testRemoteServerButtonsExposeProgressThroughDescriptorState",
  () => {
    const buttons = P.remoteButtons(false, "deploy");
    assert.deepEqual(
      buttons.map((b) => b.intent),
      [
        "save",
        "removeLocal",
        "discover",
        "deploy",
        "syncAccounts",
        "refresh",
        "start",
        "logs",
        "uninstall",
      ],
    );
    const deploy = buttons.find((b) => b.intent === "deploy");
    assert.equal(deploy.showsProgress, true);
    assert.equal(deploy.enabled, false);
  },
);
legacy(
  "ProxyPageModelTests",
  "testRemoteServerActionHelpPresentationMatchesButtonOrder",
  () => {
    assert.deepEqual(
      P.remoteHelp(P.remoteButtons(true)).map((b) => b.action),
      [
        "save",
        "removeLocal",
        "discover",
        "deploy",
        "syncAccounts",
        "refresh",
        "stop",
        "logs",
        "uninstall",
      ],
    );
  },
);
legacy(
  "ProxyPageModelTests",
  "testLocalStartProxyUsesLocalCommandServiceForImmediateSync",
  async () => {
    const calls = [],
      status = { running: true, port: 8787 };
    const model = new Proxy(async (c, a) => {
      calls.push([c, a]);
      return status;
    });
    await model.startProxy(8787);
    assert.deepEqual(calls, [["start_proxy", { port: 8787 }]]);
    assert.deepEqual(model.proxyStatus, status);
  },
);
legacy(
  "ProxyPageModelTests",
  "testAdoptDiscoveredRemoteServerSavesWithPreviousIDAndRefreshesNewIdentity",
  async () => {
    const calls = [];
    const model = new Proxy(async (c, a) => {
      calls.push([c, a]);
      return c === "save_remote" ? a.value : { running: true };
    });
    const server = {
      id: "discovered-server",
      label: "Tokyo Recovered",
      host: "1.2.3.4",
      sshPort: 22,
      sshUser: "root",
      authMode: "keyPath",
      identityFile: "~/.ssh/id_ed25519",
      remoteDir: "/srv/copool",
      listenPort: 9898,
    };
    await model.adopt(server, "server-1");
    assert.deepEqual(
      calls.map((c) => c[0]),
      ["save_remote", "remote_action"],
    );
    assert.equal(calls[0][1].previousId, "server-1");
    assert.equal(calls[0][1].value.id, "discovered-server");
    assert.deepEqual(calls[1][1], {
      id: "discovered-server",
      action: "status",
    });
  },
);
legacy(
  "ProxyPageModelTests",
  "testDeployRemoteUsesLocalCommandServiceAndSetsSuccessNotice",
  async () => {
    const calls = [],
      status = { running: true };
    const model = new Proxy(
      async (c, a) => {
        calls.push([c, a]);
        return status;
      },
      () => {},
      { schedule: () => 0 },
    );
    await model.remoteAction("server-1", "deploy", "Tokyo");
    assert.deepEqual(calls, [
      ["remote_action", { id: "server-1", action: "deploy" }],
    ]);
    assert.deepEqual(model.notice, {
      style: "success",
      key: "proxy.notice.remote_deploy_done_format",
      args: ["Tokyo"],
    });
    assert.equal(model.remoteStatuses["server-1"], status);
  },
);
legacy(
  "ProxyPageModelTests",
  "testDeployRemoteShowsDeployingThenAutoDismissesSuccessFeedback",
  async () => {
    let complete;
    const gate = new Promise((r) => (complete = r));
    const timers = [];
    const model = new Proxy(
      () => gate,
      () => {},
      {
        schedule: (fn, delay) => {
          assert.equal(delay, 80);
          timers.push(fn);
          return 1;
        },
        clearSchedule: () => {},
        successDelay: 80,
      },
    );
    const task = model.remoteAction("server-1", "deploy", "Tokyo");
    assert.equal(model.feedbacks["server-1"].state, "deploying");
    assert.equal(model.activeActions["server-1"], "deploy");
    complete({ running: true });
    await task;
    assert.equal(model.feedbacks["server-1"].state, "success");
    assert.equal(model.activeActions["server-1"], undefined);
    timers[0]();
    assert.equal(model.feedbacks["server-1"], undefined);
  },
);
legacy(
  "ProxyPageModelTests",
  "testDeployRemoteKeepsFailureFeedbackUntilDismissed",
  async () => {
    const timers = [];
    const model = new Proxy(
      async () => {
        throw Error("sudo failed");
      },
      () => {},
      { schedule: (fn) => timers.push(fn), successDelay: 20 },
    );
    await model.remoteAction("server-1", "deploy", "Tokyo");
    assert.deepEqual(model.feedbacks["server-1"], {
      state: "failure",
      message: "sudo failed",
    });
    assert.equal(timers.length, 0);
    model.dismissFeedback("server-1");
    assert.equal(model.feedbacks["server-1"], undefined);
  },
);
legacy(
  "ProxyCoordinatorTests",
  "testRemoteStatusesRunsLookupsConcurrently",
  async () => {
    let active = 0,
      maximum = 0;
    const gates = [];
    const model = new Proxy(async () => {
      active++;
      maximum = Math.max(maximum, active);
      await new Promise((r) => gates.push(r));
      active--;
      return { running: true };
    });
    const task = model.remoteStatusesFor([
      { id: "server-1" },
      { id: "server-2" },
      { id: "server-3" },
    ]);
    assert.equal(gates.length, 3);
    gates.forEach((resolve) => resolve());
    const statuses = await task;
    assert.equal(Object.keys(statuses).length, 3);
    assert(maximum >= 2);
  },
);
test("Refreshing the proxy stays local; remote servers load once", async () => {
  const calls = [];
  const model = new Proxy(async (command, args) => {
    calls.push(args?.action ? `${command}:${args.action}` : command);
    if (command === "proxy_status") return { running: false };
    if (command === "configuration")
      return {
        remoteServers: [{ id: "server-1", label: "Tokyo" }],
        proxyConfiguration: { preferredPortText: "8787", cloudflared: {} },
      };
    if (command === "tunnel_action") return { installed: true, running: false };
    if (command === "remote_action") return { running: true };
  });
  await model.refreshStatus();
  assert.equal(calls.includes("remote_action:status"), false);
  await model.loadRemoteStatuses();
  await model.loadRemoteStatuses();
  assert.equal(calls.filter((c) => c === "remote_action:status").length, 1);
});
test("A named tunnel starts only with every field filled in", () => {
  const model = new Proxy();
  model.applySnapshot({
    proxyStatus: { running: true, port: 8787 },
    cloudflaredStatus: { installed: true, running: false },
    publicAccessEnabled: true,
    cloudflaredTunnelMode: "named",
    cloudflaredNamedInput: { hostname: "api.example.test" },
  });
  assert.equal(model.canStartTunnel, false);
  Object.assign(model.tunnelConfiguration, {
    apiToken: "token",
    accountId: "account",
    zoneId: "zone",
  });
  assert.equal(model.canStartTunnel, true);
});
test("A named tunnel starts with the token saved in the Keychain", () => {
  const model = new Proxy();
  const snapshot = (hasAPIToken) =>
    model.applyLocal(
      { running: true, port: 8787 },
      {
        cloudflared: {
          enabled: true,
          tunnelMode: "named",
          namedHostname: "api.example.test",
          accountId: "account",
          zoneId: "zone",
          hasAPIToken,
        },
      },
      { installed: true, running: false },
    );
  snapshot(false);
  assert.equal(model.canStartTunnel, false);
  snapshot(true);
  assert.equal(model.tunnelConfiguration.apiToken, "");
  assert.equal(model.canStartTunnel, true);
});
test("Saving a remote card locks only that card", async () => {
  let release;
  const model = new Proxy(async (command, { value }) => {
    if (command === "save_remote")
      return new Promise((resolve) => (release = () => resolve(value)));
  });
  const task = model.saveRemoteCard({ id: "server-1", label: "Tokyo" });
  assert.equal(model.activeActions["server-1"], "save");
  assert.equal(model.remoteButtons("server-2")[0].enabled, true);
  release();
  await task;
  assert.equal(model.activeActions["server-1"], undefined);
  assert.equal(model.notice.key, "settings.notice.remote_servers_saved");
});
