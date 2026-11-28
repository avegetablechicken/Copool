const { test } = require("node:test");
const assert = require("node:assert/strict");
const { Accounts } = require("../ui/controllers.js");
const P = require("../ui/presentation.js");
const legacy = (suite, name, fn) => test(`${suite}.${name}`, fn);
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((a, b) => {
    resolve = a;
    reject = b;
  });
  return { promise, resolve, reject };
};
const account = (id, status = "list", extra = {}) => ({
  id,
  accountId: `account-${id}`,
  label: id,
  email: `${id}@example.test`,
  teamName: `${id}-space`,
  planType: "team",
  displayStatus: status,
  workspaceStatus: status === "deactivated" ? "deactivated" : "active",
  available: status === "list",
  isCurrent: id === "a",
  proxyUrl: "",
  ...extra,
});
const entry = (id, extra = {}) => ({
  workspaceId: `account-${id}`,
  workspaceName: `${id}-space`,
  kind: "workspace",
  source: "legacyMetadata",
  status: "active",
  visibility: "visible",
  ...extra,
});
function fixture(accounts = [account("a")], directory = [], handlers = {}) {
  const db = {
    accounts: structuredClone(accounts),
    directory: structuredClone(directory),
    calls: [],
    mutations: 0,
  };
  const invoke = async (command, args = {}) => {
    db.calls.push([command, structuredClone(args)]);
    if (handlers[command]) return handlers[command](args, db);
    switch (command) {
      case "accounts":
        return structuredClone(db.accounts);
      case "workspace_directory":
        return structuredClone(db.directory);
      case "cancel_workspace_refresh":
        return;
      case "refresh_usage":
        return;
      case "dismiss_workspace":
        db.directory = db.directory.map((e) =>
          e.workspaceId === args.id ? { ...e, visibility: "deleted" } : e,
        );
        return;
      case "delete_account":
        db.accounts = db.accounts.map((a) =>
          a.id === args.id ? { ...a, displayStatus: "deleted" } : a,
        );
        return;
      case "begin_login":
        return "test-login";
      case "finish_login":
        return 1;
      case "cancel_login":
        return;
      default:
        throw Error(command);
    }
  };
  const model = new Accounts(
    invoke,
    accounts,
    () => {},
    () => db.mutations++,
  );
  model.directory = structuredClone(directory);
  return { model, db };
}
legacy(
  "AccountsCoordinatorTests",
  "testCardEqualityIncludesProxyOnlyChanges",
  () => {
    const a = account("a"),
      b = { ...a, proxyUrl: "http://127.0.0.1:8080" };
    assert.equal(P.cardsEqual(a, b), false);
    assert.equal(P.cardsEqual(a, structuredClone(a)), true);
  },
);
legacy(
  "AccountsCoordinatorTests",
  "testAccountsPageModelBootstrapsFromInitialAccounts",
  () => {
    const { model } = fixture();
    assert.equal(model.loaded, true);
    assert.deepEqual(
      model.visible.map((a) => a.id),
      ["a"],
    );
    assert.equal(model.accounts[0].isCurrent, true);
  },
);
legacy(
  "AccountsCoordinatorTests",
  "testAccountsPageModelLoadIfNeededReloadsAfterAlreadyLoadedExternalMutation",
  async () => {
    const { model, db } = fixture([account("a"), account("b")]);
    await model.load();
    assert.equal(model.accounts.find((a) => a.isCurrent).id, "a");
    db.accounts.forEach((a) => (a.isCurrent = a.id === "b"));
    await model.load();
    assert.equal(model.accounts.find((a) => a.isCurrent).id, "b");
  },
);
legacy(
  "AccountsCoordinatorTests",
  "testAccountsPageModelRemoteRefreshActivityDoesNotDriveToolbarSpinner",
  () => {
    const { model } = fixture();
    assert.equal(model.spinner, false);
    assert.equal(model.canRefresh, true);
    model.syncActivity(["a"]);
    assert.equal(model.spinner, false);
    assert.equal(model.canRefresh, true);
    model.syncActivity([]);
    assert.equal(model.spinner, false);
  },
);
legacy(
  "AccountsCoordinatorTests",
  "testDeletePendingWorkspaceDoesNotReloadWorkspaceDirectoryAfterPersistingVisibility",
  async () => {
    const { model, db } = fixture([account("a")], [entry("b")]);
    await model.dismissPending("account-b");
    assert.equal(model.pending.length, 0);
    assert.equal(model.directory[0].visibility, "deleted");
    assert.equal(
      db.calls.filter((c) => c[0] === "workspace_directory").length,
      0,
    );
    assert.deepEqual(
      db.calls.map((c) => c[0]),
      ["cancel_workspace_refresh", "dismiss_workspace"],
    );
  },
);
legacy(
  "AccountsCoordinatorTests",
  "testDeletePendingWorkspaceCancelsInFlightPendingRefreshBeforeItCanRestoreCard",
  async () => {
    const gate = deferred();
    const { model, db } = fixture([account("a")], [entry("b")], {
      workspace_directory: ({ refresh }, db) =>
        refresh ? gate.promise : db.directory,
    });
    const refreshing = model.refreshPending();
    await model.dismissPending("account-b");
    gate.resolve([entry("b")]);
    await refreshing;
    assert.equal(model.pending.length, 0);
    assert.equal(model.directory[0].visibility, "deleted");
    assert.equal(
      db.calls.filter((c) => c[0] === "cancel_workspace_refresh").length,
      1,
    );
  },
);
legacy(
  "AccountsCoordinatorTests",
  "testAccountsPagePresentationDerivesPendingCardsFromUnifiedAccountStatus",
  () => {
    const { model } = fixture([
      account("a"),
      account("b", "pending"),
      account("c", "deactivated"),
      account("d", "deleted"),
    ]);
    assert.deepEqual(model.presentation().ids, ["a"]);
    assert.deepEqual(
      model.pending.map((p) => p.id),
      ["c", "b"],
    );
    assert.deepEqual(
      model.pending.map((p) => p.status),
      ["deactivated", "pending"],
    );
  },
);
legacy(
  "AccountsCoordinatorTests",
  "testAccountCardViewStateShowsReauthenticateButtonForExpiredSignIn",
  () => {
    assert.equal(
      P.needsReauthentication(
        account("a", "list", { usageError: "error.accounts.sign_in_expired" }),
      ),
      true,
    );
    // An Antigravity login whose refresh token Google refused.
    assert.equal(
      P.needsReauthentication(
        account("a", "list", { usageError: "error.antigravity.sign_in_expired" }),
      ),
      true,
    );
    assert.equal(
      P.needsReauthentication(
        account("a", "list", { usageError: "network timeout" }),
      ),
      false,
    );
    assert.equal(
      P.needsReauthentication(
        account("a", "list", {
          sourceTag: "SUB2API", providerKind: "sub2api",
          usageError: "error.accounts.sign_in_expired",
        }),
      ),
      false,
    );
  },
);
legacy(
  "AccountsCoordinatorTests",
  "testAccountsPageModelManualRefreshShowsSpinnerAndRestoresActionState",
  async () => {
    const gate = deferred();
    const { model, db } = fixture(undefined, [], {
      refresh_usage: () => gate.promise,
    });
    const refresh = model.refresh();
    assert.equal(model.spinner, true);
    assert.equal(model.canRefresh, true);
    assert.equal(await model.refresh(), false);
    assert.equal(db.calls.filter((c) => c[0] === "refresh_usage").length, 1);
    gate.resolve();
    await refresh;
    assert.equal(model.spinner, false);
    assert.equal(model.canRefresh, true);
  },
);
legacy(
  "AccountsCoordinatorTests",
  "testAccountsPageModelSkipsManualRefreshWhileBackgroundUsageRefreshIsActive",
  async () => {
    const { model, db } = fixture();
    model.syncActivity(["a"]);
    assert.equal(await model.refresh(), false);
    assert.equal(db.calls.length, 0);
  },
);
test("Single-account refresh waits only for that account", async () => {
  const gate = deferred();
  const { model, db } = fixture([account("a"), account("b")], [], {
    refresh_usage: () => gate.promise,
  });
  model.syncActivity(["a"]);
  assert.equal(await model.refresh("a"), false);
  const refreshB = model.refresh("b");
  assert.equal(await model.refresh("b"), false);
  assert.equal(await model.refresh(), false);
  gate.resolve();
  assert.equal(await refreshB, true);
  assert.deepEqual(
    db.calls.filter((c) => c[0] === "refresh_usage").map((c) => c[1]),
    [{ id: "b" }],
  );
});
legacy(
  "AccountsCoordinatorTests",
  "testAccountsPageModelLocalMutationTriggersImmediateRemoteSync",
  async () => {
    const { model, db } = fixture();
    await model.removeAccount("a");
    assert.equal(db.mutations, 1);
    assert.equal(model.visible.length, 0);
  },
);
legacy(
  "AccountsCoordinatorTests",
  "testAccountsPageModelManualRefreshReloadsAccountsFromStore",
  async () => {
    const { model } = fixture([account("a"), account("b")], [], {
      refresh_usage: (_, db) => {
        db.accounts.forEach((a) => (a.isCurrent = a.id === "b"));
      },
    });
    await model.refresh();
    assert.equal(model.accounts.find((a) => a.isCurrent).id, "b");
  },
);
legacy(
  "AccountsCoordinatorTests",
  "testAccountsPageModelReauthenticateExpiredTeamAccountUsesWorkspaceLogin",
  async () => {
    const { model, db } = fixture(
      [account("a", "list", { usageError: "error.accounts.sign_in_expired" })],
      [],
      {
        finish_login: (_, db) => {
          db.accounts[0].usageError = null;
          return 1;
        },
      },
    );
    await model.login(model.accounts[0].accountId);
    await model.discovery;
    assert.deepEqual(db.calls.find((c) => c[0] === "begin_login")[1], {
      workspace: "account-a",
    });
    assert.equal(model.accounts[0].usageError, null);
  },
);
legacy(
  "AccountsCoordinatorTests",
  "testAccountsPageLoadHidesDeletedTeamAccounts",
  async () => {
    const { model } = fixture([account("a"), account("b", "deleted")]);
    await model.load();
    assert.deepEqual(
      model.visible.map((a) => a.id),
      ["a"],
    );
    assert.equal(model.pending.length, 0);
  },
);
legacy(
  "AccountsCoordinatorTests",
  "testAccountsPageModelSingleAccountRefreshTargetsOnlyOneAccountWithoutRemoteSync",
  async () => {
    const { model, db } = fixture([account("a"), account("b")]);
    await model.refresh("b");
    assert.deepEqual(
      db.calls.filter((c) => c[0] === "refresh_usage").map((c) => c[1]),
      [{ id: "b" }],
    );
    assert.equal(db.mutations, 0);
    assert.equal(model.refreshingIds.size, 0);
  },
);
legacy(
  "AccountsCoordinatorTests",
  "testAccountsPageModelAddAccountViaLoginPopulatesPendingWorkspaceCardsFromConsentSource",
  async () => {
    const { model } = fixture([], [], {
      finish_login: (_, db) => {
        db.accounts = [account("a")];
        db.directory = [entry("b", { source: "consent" })];
        return 1;
      },
    });
    await model.login();
    await model.discovery;
    assert.deepEqual(
      model.pending.map((p) => p.workspaceId),
      ["account-b"],
    );
    assert.equal(model.pending[0].workspaceName, "b-space");
    assert.equal(model.directory[0].source, "consent");
  },
);
legacy(
  "AccountsCoordinatorTests",
  "testAccountsPageModelAddAccountViaLoginPopulatesPendingWorkspaceCardsFromWorkspaceMetadataWhenConsentUnavailable",
  async () => {
    const { model } = fixture([], [], {
      finish_login: (_, db) => {
        db.accounts = [account("a")];
        return 1;
      },
      workspace_directory: ({ refresh }) => (refresh ? [entry("b")] : []),
    });
    await model.login();
    await model.discovery;
    assert.deepEqual(
      model.pending.map((p) => p.workspaceId),
      ["account-b"],
    );
    assert.equal(model.directory[0].source, "legacyMetadata");
  },
);
async function cancellationCase(workspace) {
  const gate = deferred();
  const { model, db } = fixture([account("a")], [entry("b")], {
    finish_login: () => gate.promise,
    cancel_login: () => gate.reject("cancelled"),
  });
  const task = model.login(workspace);
  await Promise.resolve();
  await Promise.resolve();
  assert.equal(model.loginId, "test-login");
  await model.cancelLogin();
  assert.equal(await task, false);
  assert.equal(model.adding, false);
  assert.equal(model.accounts.length, 1);
  assert.equal(db.mutations, 0);
  assert.deepEqual(
    db.calls.filter((c) => c[0] === "cancel_login").map((c) => c[1]),
    [{ id: "test-login" }],
  );
}
legacy(
  "AccountsCoordinatorTests",
  "testAccountsPageModelCancelAddAccountStopsPendingLoginTask",
  () => cancellationCase(null),
);
legacy(
  "AccountsCoordinatorTests",
  "testAccountsPageModelCancelPendingWorkspaceAuthorizationStopsPendingLoginTask",
  () => cancellationCase("account-b"),
);
test("a sign-in cancelled as it begins and retried at once cancels only the first", async () => {
  // As the app's LoginState: one sign-in at a time, its commands taken in
  // turn, and a cancel ending only the sign-in it names.
  const tick = () => new Promise((resolve) => setImmediate(resolve));
  const gates = {};
  const cancelled = [];
  let n = 0;
  let live = null;
  let lock = Promise.resolve();
  const locked = (task) => {
    const result = lock.then(task);
    lock = result.catch(() => {});
    return result;
  };
  const { model, db } = fixture([account("a")], [], {
    begin_login: () => {
      const id = `login-${++n}`;
      return locked(async () => {
        if (live) throw new Error("Login already in progress");
        await new Promise((resolve) => (gates[id] = resolve));
        live = id;
        return id;
      });
    },
    cancel_login: ({ id }) =>
      locked(() => {
        if (live === id) {
          cancelled.push(id);
          live = null;
        }
      }),
    finish_login: ({ id }) =>
      locked(() => {
        if (live !== id) throw new Error("Login not found");
        live = null;
        return 1;
      }),
  });
  const first = model.login();
  await tick();
  const cancel = model.cancelLogin();
  const second = model.login();
  gates["login-1"]();
  await tick();
  await tick();
  gates["login-2"]();
  const [firstResult, , secondResult] = await Promise.allSettled([first, cancel, second]);
  assert.deepEqual(firstResult, { status: "fulfilled", value: false });
  assert.equal(secondResult.status, "fulfilled", String(secondResult.reason));
  assert.notEqual(secondResult.value, false);
  assert.deepEqual(cancelled, ["login-1"]);
  assert.deepEqual(
    db.calls.filter(([command]) => command === "cancel_login").map(([, args]) => args),
    [{ id: "login-1" }],
  );
  assert.equal(model.adding, false);
});
legacy(
  "AccountsCoordinatorTests",
  "testAccountsPageModelImportAuthDocumentAddsNewAccountWithoutChangingCurrentSelection",
  async () => {
    const { model, db } = fixture([account("a")], [], {
      import_json: ({ content }, db) => {
        assert.equal(content, "synthetic-document");
        db.accounts.push(account("b"));
        return 1;
      },
    });
    await model.importDocument("synthetic-document");
    await model.discovery;
    assert.equal(model.accounts.find((a) => a.isCurrent).id, "a");
    assert.equal(model.accounts.length, 2);
    assert.equal(db.mutations, 1);
    assert.equal(
      db.calls.some((c) => c[0] === "switch_account"),
      false,
    );
  },
);
legacy(
  "AccountsCoordinatorTests",
  "testAccountsPageModelImportAuthFileImportsSelectedDocumentWithoutChangingCurrentSelection",
  async () => {
    const { model, db } = fixture([account("a")], [], {
      import_account_file: (_, db) => {
        db.accounts.push(account("b"));
        return 1;
      },
    });
    await model.importFile();
    await model.discovery;
    assert.equal(model.accounts.find((a) => a.isCurrent).id, "a");
    assert.equal(model.accounts.length, 2);
    assert.equal(db.mutations, 1);
    assert.equal(
      db.calls.some((c) => c[0] === "switch_account"),
      false,
    );
  },
);
legacy(
  "AccountsCoordinatorTests",
  "testAccountsPageModelAuthorizePendingWorkspaceImportsAndClearsCandidate",
  async () => {
    const { model, db } = fixture([account("a")], [entry("b")], {
      finish_login: (_, db) => {
        db.accounts.push(account("b"));
        db.directory = [];
        return 1;
      },
    });
    await model.login("account-b");
    await model.discovery;
    assert.equal(model.pending.length, 0);
    assert.equal(model.accounts.length, 2);
    assert.equal(db.mutations, 1);
  },
);
legacy(
  "AccountsCoordinatorTests",
  "testAccountsPageModelPendingWorkspaceDiscoveryShowsErrorWhenMetadataLookupFails",
  async () => {
    const { model } = fixture([account("a")], [], {
      workspace_directory: () => {
        throw "Provided authentication token is expired. Please try signing in again.";
      },
    });
    await model.refreshPending();
    assert.equal(model.pending.length, 0);
    assert.equal(
      model.pendingError,
      "Provided authentication token is expired. Please try signing in again.",
    );
  },
);
async function deactivatedLogin(error) {
  const { model } = fixture([account("a")], [entry("b")], {
    finish_login: () => {
      throw error;
    },
  });
  await assert.rejects(model.login("account-b"));
  assert.equal(model.accounts.length, 1);
  assert.deepEqual(
    model.pending.map((p) => p.workspaceId),
    ["account-b"],
  );
  assert.equal(model.pending[0].status, "deactivated");
  assert.equal(model.notice, "error.accounts.workspace_deactivated");
}
legacy(
  "AccountsCoordinatorTests",
  "testAccountsPageModelAuthorizePendingWorkspaceKeepsCandidateWhenWorkspaceIsDeactivated",
  () => deactivatedLogin("This workspace is deactivated"),
);
legacy(
  "AccountsCoordinatorTests",
  "testAccountsPageModelAuthorizePendingWorkspaceKeepsCandidateWhenWorkspaceUsesTypedDeactivatedError",
  () => deactivatedLogin({ code: "error.accounts.workspace_deactivated" }),
);
async function deactivateRefresh(error) {
  const { model } = fixture([account("a")], [], {
    refresh_usage: (_, db) => {
      db.accounts[0].workspaceStatus = "deactivated";
      db.accounts[0].displayStatus = "deactivated";
      throw error;
    },
  });
  await assert.rejects(model.refresh("a"));
  assert.equal(model.visible.length, 0);
  assert.equal(model.pending[0].id, "a");
  assert.equal(model.pending[0].status, "deactivated");
}
legacy(
  "AccountsCoordinatorTests",
  "testAccountsPageModelRefreshUsageMovesDeactivatedAccountIntoPendingSection",
  () => deactivateRefresh("workspace_deactivated"),
);
legacy(
  "AccountsCoordinatorTests",
  "testAccountsPageModelRefreshUsageMovesDeactivatedAccountIntoPendingSectionForTypedError",
  () => deactivateRefresh({ code: "error.accounts.workspace_deactivated" }),
);
legacy(
  "AccountsCoordinatorTests",
  "testPendingWorkspaceCardRulesSortDeactivatedBeforeNameOrder",
  () => {
    const { model } = fixture([
      account("z", "deactivated", { teamName: "Z dead" }),
      account("b", "pending", { teamName: "Beta" }),
      account("a", "pending", { teamName: "Alpha" }),
    ]);
    assert.deepEqual(
      model.pending.map((a) => a.id),
      ["z", "a", "b"],
    );
  },
);
legacy(
  "AccountsCoordinatorTests",
  "testAccountsPageModelDeletePendingWorkspaceMarksDeactivatedAccountDeleted",
  async () => {
    const { model, db } = fixture([account("a", "deactivated")]);
    await model.dismissPending("a");
    assert.equal(model.accounts[0].displayStatus, "deleted");
    assert.equal(model.pending.length, 0);
    assert.equal(db.accounts[0].displayStatus, "deleted");
  },
);
legacy(
  "AccountsCoordinatorTests",
  "testAccountsPageModelLoadShowsOnlyUnauthorizedWorkspaceCandidatesFromWorkspaceDirectory",
  async () => {
    const { model } = fixture(
      [account("a")],
      [entry("a"), entry("b"), entry("c", { visibility: "deleted" })],
    );
    await model.load();
    assert.deepEqual(
      model.pending.map((p) => p.workspaceId),
      ["account-b"],
    );
  },
);
legacy(
  "AccountsCoordinatorTests",
  "testAccountsPageModelLoadDoesNotMoveAuthCardIntoPendingSectionWithoutStoredDeactivatedDirectoryEntry",
  async () => {
    const { model } = fixture([account("a")]);
    await model.load();
    assert.deepEqual(
      model.visible.map((a) => a.id),
      ["a"],
    );
    assert.equal(model.pending.length, 0);
  },
);
legacy(
  "AccountsCoordinatorTests",
  "testAccountsPageModelLoadDoesNotShowDeletedTeamAccountFromWorkspaceDirectoryEntry",
  async () => {
    const { model } = fixture([account("a", "deleted")], [entry("a")]);
    await model.load();
    assert.equal(model.visible.length, 0);
    assert.equal(model.pending.length, 0);
  },
);
legacy(
  "AccountsCoordinatorTests",
  "testAccountsPageModelPendingWorkspaceRefreshUsesWorkspaceMetadataServiceForUnauthorizedDiscovery",
  async () => {
    const { model, db } = fixture([account("a")], [], {
      workspace_directory: ({ refresh }) => (refresh ? [entry("b")] : []),
    });
    await model.refreshPending();
    assert.deepEqual(
      model.pending.map((p) => p.workspaceId),
      ["account-b"],
    );
    assert.deepEqual(db.calls, [["workspace_directory", { refresh: true }]]);
  },
);
legacy(
  "AccountsCoordinatorTests",
  "testAccountsPageModelBackgroundRefreshDoesNotTouchPendingWorkspaceDiscovery",
  () => {
    const { model, db } = fixture([account("a")], [entry("b")]);
    model.accept([account("a", "list", { usage: { fetchedAt: 9 } })]);
    assert.equal(db.calls.length, 0);
    assert.deepEqual(
      model.pending.map((p) => p.workspaceId),
      ["account-b"],
    );
  },
);
legacy(
  "AccountsCoordinatorTests",
  "testAccountsPageModelPendingWorkspaceDiscoveryCancellationDoesNotShowError",
  async () => {
    const { model } = fixture([account("a")], [], {
      workspace_directory: () => {
        throw "request cancelled";
      },
    });
    await model.refreshPending();
    assert.equal(model.pendingError, null);
  },
);
legacy(
  "AccountsCoordinatorTests",
  "testAccountsPageModelPendingWorkspaceDiscoveryStopsAfterCancellation",
  async () => {
    const gate = deferred();
    const { model, db } = fixture([account("a")], [], {
      workspace_directory: () => gate.promise,
    });
    const task = model.refreshPending();
    await model.cancelPending();
    gate.resolve([entry("b")]);
    await task;
    assert.equal(model.pending.length, 0);
    assert.equal(model.pendingError, null);
    assert.equal(
      db.calls.filter((c) => c[0] === "cancel_workspace_refresh").length,
      1,
    );
  },
);
legacy(
  "AccountsCoordinatorTests",
  "testAccountsPageModelDeletePendingWorkspacePersistsDismissalForStoredDeactivatedEntry",
  async () => {
    const { model, db } = fixture(
      [account("a")],
      [entry("b", { status: "deactivated" })],
    );
    await model.dismissPending("account-b");
    assert.equal(db.directory[0].visibility, "deleted");
    assert.equal(model.pending.length, 0);
  },
);
legacy(
  "AccountsCoordinatorTests",
  "testDeletingDeactivatedPendingAccountRemovesCardImmediately",
  async () => {
    const gate = deferred();
    const { model } = fixture([account("a", "deactivated")], [], {
      delete_account: () => gate.promise,
    });
    const task = model.dismissPending("a");
    assert.equal(model.pending.length, 0);
    gate.resolve();
    await task;
    assert.equal(model.accounts[0].displayStatus, "deleted");
  },
);
legacy(
  "AccountsCoordinatorTests",
  "testContentPresentationHidesPendingSectionForErrorWithoutCards",
  () => {
    const { model } = fixture();
    model.pendingError = "metadata forbidden";
    assert.deepEqual(model.presentation().pending, []);
    assert.equal(model.presentation().pendingError, null);
    assert.equal(model.presentation().showPending, false);
  },
);
legacy(
  "AccountsCoordinatorTests",
  "testTrayMenuModelSyncLocalMutationAlsoSyncsConfiguredRemoteAccounts",
  async () => {
    const { model, db } = fixture([account("a")], [], {
      import_current: (_, db) => {
        db.accounts.push(account("b"));
        return 1;
      },
    });
    await model.importCurrent();
    await model.discovery;
    assert.equal(db.mutations, 1);
    assert.equal(model.accounts.length, 2);
  },
);
test("Imports report the imported labels and a cancelled picker reports nothing", async () => {
  const { model, db } = fixture([account("a")], [], {
    import_account_file: (_, db) => {
      db.accounts.push(account("b"));
      return 1;
    },
  });
  const states = [];
  model.onUpdate = () => states.push(model.importing);
  assert.deepEqual(await model.importFile(), ["b"]);
  assert.equal(states.includes(true), true);
  assert.equal(model.importing, false);
  db.mutations = 0;
  const cancelled = fixture([account("a")], [], {
    import_account_file: () => null,
  });
  assert.equal(await cancelled.model.importFile(), null);
  assert.equal(cancelled.db.mutations, 0);
});
test("Authorizing a pending workspace does not turn Add into Cancel", async () => {
  let release;
  const { model } = fixture([account("a")], [], {
    finish_login: () => new Promise((resolve) => (release = resolve)),
  });
  const task = model.login("account-b", () => {}, "pending-b");
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(model.adding, false);
  assert.equal(model.authorizing, "pending-b");
  assert.equal(await model.login(), false);
  release(1);
  await task;
  assert.equal(model.authorizing, null);
});
test("A provider refresh locks only its cards and refreshes them in one call", async () => {
  const gate = deferred();
  const { model, db } = fixture([account("a"), account("p1"), account("p2")], [], {
    refresh_provider: () => gate.promise,
  });
  const running = model.refreshProvider("team", ["p1", "p2"]);
  assert.deepEqual([...model.refreshingIds], ["p1", "p2"]);
  // The provider's cards are busy; another account is not.
  assert.equal(await model.refreshProvider("team", ["p2"]), false);
  assert.equal(model.refreshingIds.has("a"), false);
  gate.resolve();
  assert.equal(await running, true);
  assert.equal(model.refreshingIds.size, 0);
  assert.deepEqual(
    db.calls.filter(([command]) => command === "refresh_provider"),
    [["refresh_provider", { id: "team", sync: false }]],
  );
});
test("A provider's cards are removed together with one reload", async () => {
  const { model, db } = fixture([account("a"), account("p1"), account("p2")], [], {
    delete_account: (args, db) => {
      db.accounts = db.accounts.filter((a) => a.id !== args.id);
    },
  });
  await model.removeAccounts(["p1", "p2"]);
  assert.deepEqual(model.visible.map((a) => a.id), ["a"]);
  assert.equal(db.calls.filter(([command]) => command === "accounts").length, 1);
  assert.equal(db.mutations, 1);
});

test("A load answered after a later one leaves the later one's cards", async () => {
  // The app renders the controller's cards once its other requests are in,
  // so an older refresh never shows the previous CURRENT tag again.
  const answers = [];
  const invoke = (command) =>
    command === "accounts" ? new Promise((resolve) => answers.push(resolve)) : Promise.resolve([]);
  const model = new Accounts(invoke, []);
  const older = model.load();
  const newer = model.load();
  await new Promise((resolve) => setImmediate(resolve));
  answers[1]([{ id: "a", isCurrent: false }]);
  await newer;
  answers[0]([{ id: "a", isCurrent: true }]);
  assert.deepEqual(await older, [{ id: "a", isCurrent: false }]);
  assert.equal(model.accounts[0].isCurrent, false);
});

test("A page refresh shows the cards of a load finished while its other requests were pending", async () => {
  // As app.js's refreshView: an older refresh, slow on another request,
  // never shows the previous CURRENT tag again.
  let current = true;
  const invoke = async (command) => (command === "accounts" ? [{ id: "a", isCurrent: current }] : []);
  const model = new Accounts(invoke, []);
  let finish;
  const slow = new Promise((resolve) => (finish = resolve));
  const older = model.loadAlong([slow, "status"]);
  await new Promise((resolve) => setImmediate(resolve));
  current = false;
  const newer = await model.loadAlong([]);
  assert.equal(newer.accounts[0].isCurrent, false);
  finish("feature state");
  const { accounts, answers } = await older;
  assert.equal(accounts[0].isCurrent, false);
  assert.deepEqual(answers, ["feature state", "status"]);
});

test("Refreshing one of a provider's cards shows all of them refreshing", async () => {
  let finish;
  const invoke = (command) =>
    command === "refresh_provider" ? new Promise((resolve) => (finish = resolve)) : Promise.resolve([]);
  const cards = [
    { id: "p1", refresh: { key: "sub2api:team" } },
    { id: "p2", refresh: { key: "sub2api:team" } },
    { id: "own", refresh: { key: "own" } },
  ];
  const model = new Accounts(invoke, structuredClone(cards));
  const refreshing = model.refreshCard("p1");
  assert.deepEqual([...model.refreshingIds].sort(), ["p1", "p2"]);
  // Its sibling, refreshing with it, refreshes no second time.
  assert.equal(await model.refreshCard("p2"), false);
  finish();
  assert.equal(await refreshing, true);
  assert.equal(model.refreshingIds.size, 0);
});

test("Refreshing a provider's only card refreshes the entire provider", async () => {
  const calls = [];
  const invoke = async (command, args) => {
    if (command === "refresh_usage" || command === "refresh_provider") calls.push([command, args]);
    return [];
  };
  // The cards shown, with the refresh keys the core gives them.
  const cards = [
    { id: "claude-sub2api-team-7", refresh: { key: "sub2api:team" } },
    { id: "sub2api-work-42", refresh: { key: "sub2api:work" } },
    { id: "official", refresh: { key: "official" } },
  ];
  for (const id of ["claude-sub2api-team-7", "sub2api-work-42", "official", null])
    await new Accounts(invoke, structuredClone(cards)).refreshCard(id);
  assert.deepEqual(calls, [
    ["refresh_provider", { id: "team", sync: false }],
    ["refresh_provider", { id: "work", sync: false }],
    ["refresh_usage", { id: "official" }],
    ["refresh_usage", { id: null }],
  ]);
});
test("a sign-in in another pool waits for one cancelled as it begins", async () => {
  // The app's one LoginState, shared by every pool.
  const tick = () => new Promise((resolve) => setImmediate(resolve));
  const gates = {};
  const calls = [];
  let n = 0;
  let live = null;
  let lock = Promise.resolve();
  const locked = (task) => {
    const result = lock.then(task);
    lock = result.catch(() => {});
    return result;
  };
  const invoke = async (command, args) => {
    calls.push([command, args]);
    if (command === "begin_login") {
      const id = `login-${++n}`;
      return locked(async () => {
        if (live) throw new Error("Login already in progress");
        await new Promise((resolve) => (gates[id] = resolve));
        live = id;
        return id;
      });
    }
    if (command === "cancel_login")
      return locked(() => {
        if (live === args.id) live = null;
      });
    if (command === "finish_login")
      return locked(() => {
        if (live !== args.id) throw new Error("Login not found");
        live = null;
        return 1;
      });
    return [];
  };
  const signIns = { cancel: null };
  const codex = new Accounts(invoke, [], () => {}, () => {}, { pool: "codex", signIns });
  const claude = new Accounts(invoke, [], () => {}, () => {}, { pool: "claude", signIns });
  const first = codex.login();
  await tick();
  const cancel = codex.cancelLogin();
  const second = claude.login();
  gates["login-1"]();
  await tick();
  await tick();
  gates["login-2"]?.();
  await tick();
  gates["login-2"]?.();
  const [firstResult, , secondResult] = await Promise.allSettled([first, cancel, second]);
  assert.deepEqual(firstResult, { status: "fulfilled", value: false });
  assert.equal(secondResult.status, "fulfilled", String(secondResult.reason));
  assert.deepEqual(
    calls.map(([command]) => command).filter((c) => c.endsWith("_login")),
    ["begin_login", "cancel_login", "begin_login", "finish_login"],
  );
});
test("cancelling a sign-in that waits for another pool's cancel keeps the app waiting for it", async () => {
  const tick = () => new Promise((resolve) => setImmediate(resolve));
  const gates = {};
  let n = 0;
  let live = null;
  let lock = Promise.resolve();
  const locked = (task) => {
    const result = lock.then(task);
    lock = result.catch(() => {});
    return result;
  };
  const invoke = async (command, args) => {
    if (command === "begin_login") {
      const id = `login-${++n}`;
      return locked(async () => {
        if (live) throw new Error("Login already in progress");
        await new Promise((resolve) => (gates[id] = resolve));
        live = id;
        return id;
      });
    }
    if (command === "cancel_login")
      return locked(() => {
        if (live === args.id) live = null;
      });
    if (command === "finish_login")
      return locked(() => {
        if (live !== args.id) throw new Error("Login not found");
        live = null;
        return 1;
      });
    return [];
  };
  const signIns = { cancel: null };
  const codex = new Accounts(invoke, [], () => {}, () => {}, { pool: "codex", signIns });
  const claude = new Accounts(invoke, [], () => {}, () => {}, { pool: "claude", signIns });
  const first = codex.login();
  await tick();
  const cancelFirst = codex.cancelLogin();
  // Claude's sign-in waits for Codex's cancel, and is cancelled meanwhile.
  const second = claude.login();
  await claude.cancelLogin();
  const third = claude.login();
  await tick();
  // Nothing begins while Codex's sign-in is being cancelled.
  assert.equal(n, 1);
  gates["login-1"]();
  for (let i = 0; i < 6; i++) {
    await tick();
    gates["login-2"]?.();
  }
  const results = await Promise.allSettled([first, cancelFirst, second, third]);
  assert.deepEqual(results[2], { status: "fulfilled", value: false });
  assert.equal(results[3].status, "fulfilled", String(results[3].reason));
  assert.notEqual(results[3].value, false);
});
