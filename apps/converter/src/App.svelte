<script lang="ts">
  import { onMount } from "svelte";
  import { ConverterController } from "./controller";
  import type {
    BrowserFile,
    ConverterState,
    Diagnostic,
    Family,
    FormatInfo,
    Job,
  } from "./types";

  const base = import.meta.env.BASE_URL;
  const families: Family[] = ["transmission", "distribution"];
  let controller = $state.raw<ConverterController | null>(null);
  let view = $state.raw<ConverterState>({
    formats: [],
    engine: { version: "", commit: "" },
    jobs: [],
    targets: { transmission: ["matpower"], distribution: ["pmd-json"] },
    phase: "loading",
    activeName: "",
    message: "",
    error: "",
    analytics: false,
  });
  let fileInput: HTMLInputElement;
  let folderInput: HTMLInputElement;
  let missingInput: HTMLInputElement;
  let missingJobId: string | undefined;
  let reportDialog: HTMLDialogElement;
  let privacyDetails: HTMLDetailsElement;
  let report = $state("");
  let dragging = $state(false);
  let notice = $state("");
  let uiError = $state("");
  let formatSearch = $state("");
  let targetSearch = $state("");
  let dragDepth = 0;
  let busy = $derived(view.phase !== "idle" && view.phase !== "loading");
  let hasFiles = $derived(view.jobs.length > 0);
  let outputs = $derived(
    view.jobs
      .flatMap((job) => job.outputs)
      .filter((output) => output.status !== "error"),
  );
  let failures = $derived(
    view.jobs.filter(
      (job) =>
        job.status === "error" ||
        job.status === "cancelled" ||
        job.status === "needs-files" ||
        job.outputs.some((output) => output.status === "error"),
    ),
  );
  let ready = $derived(
    view.jobs.filter((job) => job.status === "ready" || job.status === "done"),
  );
  let plannedCount = $derived(
    ready.reduce(
      (sum, job) =>
        sum +
        (job.targets ?? (job.family ? view.targets[job.family] : [])).filter(
          (token) => {
            const format = view.formats.find(
              (candidate) => candidate.token === token,
            );
            return format && formatAvailable(format, job);
          },
        ).length,
      0,
    ),
  );
  let outputFormats = $derived(
    view.formats.filter(
      (format) =>
        format.canEmit &&
        `${format.label} ${format.token}`
          .toLowerCase()
          .includes(targetSearch.toLowerCase()),
    ),
  );
  let activeFamilies = $derived(
    families.filter(
      (family) => !hasFiles || view.jobs.some((job) => job.family === family),
    ),
  );
  let terminalCommands = $derived(getCommands(view));
  let visibleFormats = $derived(
    view.formats.filter((format) =>
      `${format.label} ${format.token}`
        .toLowerCase()
        .includes(formatSearch.toLowerCase()),
    ),
  );

  onMount(() => {
    const current = new ConverterController();
    controller = current;
    const unsubscribe = current.subscribe((next: ConverterState) => {
      view = next;
    });
    void current.initialize().catch((error: unknown) => {
      uiError = errorMessage(error);
    });
    return () => {
      unsubscribe();
      current.dispose();
    };
  });

  function formatAvailable(format: FormatInfo, job?: Job) {
    if (!format.requiresValueType) return true;
    if (job)
      return (
        job.family === format.family &&
        job.valueType === format.requiresValueType
      );
    return view.jobs.some(
      (candidate) =>
        candidate.family === format.family &&
        candidate.valueType === format.requiresValueType,
    );
  }
  function formatRequirement(format: FormatInfo) {
    return format.requiresValueType === "powerio.AcScucSolution"
      ? "Requires a complete SCUC solution"
      : `Requires ${format.requiresValueType}`;
  }
  function getCommands(_view: ConverterState) {
    return (
      controller?.commands() ??
      "cargo install powerio-cli\npowerio convert case.raw --to matpower -o case.m"
    );
  }
  function errorMessage(error: unknown) {
    return error instanceof Error
      ? error.message
      : "Something interrupted this action. Please try again.";
  }
  async function action(run: () => unknown | Promise<unknown>) {
    uiError = "";
    notice = "";
    try {
      await run();
    } catch (error) {
      uiError = errorMessage(error);
    }
  }
  function formatLabel(token?: string) {
    return (
      view.formats.find((format) => format.token === token)?.label ??
      token ??
      "Detecting format"
    );
  }
  function bytes(size: number) {
    if (size < 1024) return `${size} B`;
    if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} KB`;
    return `${(size / (1024 * 1024)).toFixed(1)} MB`;
  }
  function status(job: Job) {
    if (job.status === "done") {
      if (job.outputs.some((output) => output.status === "error"))
        return { text: "Some outputs failed", tone: "warning" };
      if (
        job.outputs.some((output) => output.status === "warnings") ||
        job.diagnostics.some((item) => item.severity === "warning")
      )
        return { text: "Converted with warnings", tone: "warning" };
      if (
        job.outputs.length > 0 &&
        job.outputs.every((output) => output.status === "unchanged")
      )
        return { text: "Unchanged", tone: "success" };
      return { text: "Converted", tone: "success" };
    }
    const labels = {
      queued: "Queued",
      inspecting: "Detecting format",
      ready: "Ready to convert",
      converting: "Converting",
      error: "Couldn't convert",
      cancelled: "Cancelled",
      "needs-primary": "Choose an entry file",
      "needs-files": "Needs files",
    };
    return {
      text: labels[job.status],
      tone:
        job.status === "error"
          ? "error"
          : job.status === "needs-primary" || job.status === "needs-files"
            ? "warning"
            : "neutral",
    };
  }
  function toggleTarget(family: Family, token: string, checked: boolean) {
    const selected = view.targets[family];
    controller?.setTargets(
      family,
      checked
        ? [...selected, token]
        : selected.filter((item) => item !== token),
    );
  }
  function toggleJobTarget(job: Job, token: string, checked: boolean) {
    const selected =
      job.targets ?? (job.family ? view.targets[job.family] : []);
    controller?.setJobTargets(
      job.id,
      checked
        ? [...selected, token]
        : selected.filter((item) => item !== token),
    );
  }
  async function addSelection(event: Event) {
    const input = event.currentTarget as HTMLInputElement;
    const files = Array.from(input.files ?? []);
    await action(() => controller?.addFiles(files));
    input.value = "";
  }
  async function addMissingSelection(event: Event) {
    const input = event.currentTarget as HTMLInputElement;
    const files = Array.from(input.files ?? []);
    if (missingJobId)
      await action(() => controller?.addMissingFiles(missingJobId!, files));
    input.value = "";
    missingJobId = undefined;
  }
  function chooseMissingFiles(id: string) {
    missingJobId = id;
    missingInput.click();
  }
  interface DropEntry {
    isFile: boolean;
    name: string;
    file: (
      success: (file: File) => void,
      failure: (error: DOMException) => void,
    ) => void;
    createReader: () => {
      readEntries: (
        success: (entries: DropEntry[]) => void,
        failure: (error: DOMException) => void,
      ) => void;
    };
  }
  async function readEntry(
    entry: DropEntry,
    prefix = "",
  ): Promise<BrowserFile[]> {
    const path = `${prefix}${entry.name}`;
    if (entry.isFile) {
      const file = await new Promise<File>((resolve, reject) =>
        entry.file(resolve, reject),
      );
      return [{ path, file }];
    }
    const reader = entry.createReader();
    const result: BrowserFile[] = [];
    while (true) {
      const children = await new Promise<DropEntry[]>((resolve, reject) =>
        reader.readEntries(resolve, reject),
      );
      if (!children.length) return result;
      for (const child of children)
        result.push(...(await readEntry(child, `${path}/`)));
    }
  }
  function dragEnter(event: DragEvent) {
    if (!event.dataTransfer?.types.includes("Files")) return;
    event.preventDefault();
    dragDepth += 1;
    dragging = true;
  }
  function dragLeave(event: DragEvent) {
    event.preventDefault();
    dragDepth = Math.max(0, dragDepth - 1);
    dragging = dragDepth > 0;
  }
  async function drop(event: DragEvent) {
    event.preventDefault();
    dragDepth = 0;
    dragging = false;
    if (!event.dataTransfer || busy || view.phase === "loading") return;
    const transfer = event.dataTransfer;
    const items = Array.from(transfer.items);
    const entries = items
      .map((item) =>
        (
          item as unknown as { webkitGetAsEntry?: () => DropEntry | null }
        ).webkitGetAsEntry?.(),
      )
      .filter((entry): entry is DropEntry => !!entry);
    const files = Array.from(transfer.files);
    await action(async () => {
      if (entries.length) {
        const collected: BrowserFile[] = [];
        for (const entry of entries)
          collected.push(...(await readEntry(entry)));
        await controller?.addEntries(collected);
      } else await controller?.addFiles(files);
    });
  }
  async function copy(text: string, message = "Copied to clipboard.") {
    await action(async () => {
      await navigator.clipboard.writeText(text);
      notice = message;
    });
  }
  function openReport(jobId?: string) {
    report = controller?.issueReport(jobId) ?? "";
    reportDialog.showModal();
  }
  function reportUrl() {
    return `https://github.com/eigenergy/powerio/issues/new?title=Conversion%20report&body=${encodeURIComponent(report)}`;
  }
</script>

{#snippet icon(
  name:
    | "upload"
    | "arrow"
    | "download"
    | "lock"
    | "file"
    | "check"
    | "close"
    | "share"
    | "code",
)}
  <svg
    width="20"
    height="20"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="1.7"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
  >
    {#if name === "upload"}<path
        d="M12 16V3m-5 5 5-5 5 5M4 16v4a1 1 0 0 0 1 1h14a1 1 0 0 0 1-1v-4"
      />
    {:else if name === "arrow"}<path d="M5 12h14m-6-6 6 6-6 6" />
    {:else if name === "download"}<path
        d="M12 3v13m-5-5 5 5 5-5M4 17v3a1 1 0 0 0 1 1h14a1 1 0 0 0 1-1v-3"
      />
    {:else if name === "lock"}<rect
        x="5"
        y="10"
        width="14"
        height="11"
        rx="2"
      /><path d="M8 10V7a4 4 0 0 1 8 0v3m-4 5v2" />
    {:else if name === "file"}<path
        d="M14 2H5a1 1 0 0 0-1 1v18a1 1 0 0 0 1 1h14a1 1 0 0 0 1-1V8Zm0 0v6h6M8 13h8m-8 4h5"
      />
    {:else if name === "check"}<path d="m5 12 4 4L19 6" />
    {:else if name === "close"}<path d="m6 6 12 12M6 18 18 6" />
    {:else if name === "share"}<path d="M12 16V3m-4 4 4-4 4 4M5 13v7h14v-7" />
    {:else}<path d="m8 6-6 6 6 6m8-12 6 6-6 6m-2-15-4 18" />{/if}
  </svg>
{/snippet}

{#snippet diagnostics(items: Diagnostic[])}
  {#if items.length}
    <details class="diagnostics">
      <summary
        >{items.some((item) => item.severity === "error")
          ? "What needs attention"
          : items.some((item) => item.severity === "warning")
            ? "Review conversion warnings"
            : "Conversion details"}
        <span class="count">{items.length}</span></summary
      >
      <ol class="diagnostic-list">
        {#each items as item, index (`${item.code}-${index}`)}
          <li class={["diagnostic", item.severity]}>
            <div class="diagnostic-meta">
              <strong
                >{item.severity === "warning"
                  ? "Warning"
                  : item.severity === "error"
                    ? "Error"
                    : "Note"}</strong
              ><code>{item.code}</code>{#if item.target}<span
                  >{formatLabel(item.target)}</span
                >{/if}
            </div>
            <p>{item.message}</p>
            {#if item.suggestedAction}<p class="suggestion">
                {item.suggestedAction}
              </p>{/if}
            {#if item.spans?.length}<ul class="source-spans">
                {#each item.spans as span, index (`${span.source}-${span.start}-${index}`)}<li
                  >
                    {span.source}, bytes {span.start} to {span.end}
                  </li>{/each}
              </ul>{/if}
          </li>
        {/each}
      </ol>
    </details>
  {/if}
{/snippet}

<a href="#converter" class="skip-link">Skip to converter</a>
<header class="site-header">
  <a class="brand" href="https://powerio.dev/" aria-label="PowerIO home"
    ><img src={`${base}powerio-logo.svg`} alt="" width="35" height="35" /><span
      >PowerIO<span class="brand-product"> / Convert</span></span
    ></a
  >
  <nav aria-label="Main navigation">
    <a href="#formats">Formats</a><a
      href="https://powerio.dev/guide/getting-started.html">Docs</a
    ><a class="github-link" href="https://github.com/eigenergy/powerio"
      >GitHub {@render icon("arrow")}</a
    >
  </nav>
</header>

<main>
  <section class={["hero", hasFiles && "compact"]} aria-labelledby="page-title">
    <div class="hero-copy">
      <h1 id="page-title">
        Convert power<br class="desktop-break" /> system files<span
          class="heading-dot">.</span
        >
      </h1>
      <p class="hero-description">
        From one format to the next. Bring a file, a folder, or a whole batch.
      </p>
      <p class="privacy-promise">
        {@render icon("lock")}<span
          >Your files stay on your computer.<br class="mobile-break" /> PowerIO converts
          them in your browser.</span
        >
      </p>
      <p class="analytics-invitation">
        <a
          href="#privacy"
          onclick={() => {
            privacyDetails.open = true;
          }}>Help improve PowerIO</a
        >
        <span>Optional statistics, off by default.</span>
      </p>
    </div>
    <div class="mascot-scene" aria-hidden="true">
      <div class="mascot-orbit orbit-one"></div>
      <div class="mascot-orbit orbit-two"></div>
      <span class="format-float float-one">.raw</span><span
        class="format-float float-two">.m</span
      ><span class="format-float float-three">.dss</span><img
        class={["mascot", busy && "working"]}
        src={`${base}powerio-logo.svg`}
        alt=""
        width="210"
        height="210"
      /><span class="mascot-caption"
        >{busy
          ? "One case at a time."
          : outputs.length
            ? "Ready for the next step."
            : hasFiles
              ? "Let's get these connected."
              : "Different formats. Same grid."}</span
      >
    </div>
  </section>

  <section
    id="converter"
    class="converter"
    aria-label="Power system file converter"
  >
    <input
      class="visually-hidden"
      bind:this={missingInput}
      type="file"
      multiple
      tabindex="-1"
      aria-label="Add missing project files"
      onchange={addMissingSelection}
    />
    <input
      class="visually-hidden"
      bind:this={fileInput}
      type="file"
      multiple
      tabindex="-1"
      aria-label="Choose power system files"
      onchange={addSelection}
    />
    <input
      class="visually-hidden"
      bind:this={folderInput}
      type="file"
      multiple
      webkitdirectory
      tabindex="-1"
      aria-label="Choose a power system folder"
      onchange={addSelection}
    />
    <section
      class={["drop-zone", hasFiles && "has-files", dragging && "dragging"]}
      aria-label="Add files by dragging here or using the file buttons"
      ondragenter={dragEnter}
      ondragleave={dragLeave}
      ondragover={(event) => {
        event.preventDefault();
        if (event.dataTransfer) event.dataTransfer.dropEffect = "copy";
      }}
      ondrop={drop}
    >
      <div class="drop-symbol">{@render icon("upload")}</div>
      <div class="drop-copy">
        <h2>
          {dragging
            ? "Drop them here."
            : hasFiles
              ? "Add to your batch"
              : "Your files, in a new format."}
        </h2>
        <p>
          {hasFiles
            ? "Drop files, folders, or ZIP archives here."
            : "Drop files or folders here. Mixed formats welcome."}
        </p>
      </div>
      <div class="drop-actions">
        <button
          class="button primary"
          disabled={busy || view.phase === "loading"}
          onclick={() => fileInput.click()}
          >Choose files {@render icon("arrow")}</button
        ><button
          class="button secondary"
          disabled={busy || view.phase === "loading"}
          onclick={() => folderInput.click()}>Choose folder</button
        >
      </div>
      {#if !hasFiles}<p class="drop-footnote">
          ZIP archives work too. Formats are detected automatically.
        </p>{/if}
    </section>
    <div class="examples">
      <span>Just exploring? Try an example:</span><button
        disabled={busy || view.phase === "loading"}
        onclick={() => action(() => controller?.addExample("transmission"))}
        >Transmission</button
      ><button
        disabled={busy || view.phase === "loading"}
        onclick={() => action(() => controller?.addExample("distribution"))}
        >Distribution</button
      ><button
        disabled={busy || view.phase === "loading"}
        onclick={() => action(() => controller?.addExample("mixed"))}
        >A mixed batch {@render icon("arrow")}</button
      >
    </div>

    <div class="live-status" role="status" aria-live="polite">
      {#if view.phase === "loading"}<p class="status-message">
          <span class="spinner"></span>Getting PowerIO ready in your browser...
        </p>
      {:else if busy}<p class="status-message">
          <span class="spinner"></span><span
            >{view.phase === "inspecting"
              ? "Inspecting"
              : view.phase === "packaging"
                ? "Preparing download"
                : "Converting"}{view.activeName
              ? `: ${view.activeName}`
              : ""}</span
          >
        </p>
      {:else if notice || view.message}<p class="status-message">
          {@render icon("check")}{notice || view.message}
        </p>{/if}
    </div>
    {#if uiError || view.error}<div class="error-banner" role="alert">
        <strong>This action needs attention.</strong>
        <p>{uiError || view.error}</p>
        {#if !view.engine.version}<button
            class="button secondary"
            onclick={() => window.location.reload()}>Reload converter</button
          >{/if}
        <button class="text-button" onclick={() => openReport()}
          >Get help with this conversion</button
        >
      </div>{/if}

    {#if hasFiles}
      <div class="workspace">
        <section class="batch" aria-labelledby="batch-title">
          <div class="section-heading">
            <div>
              <h2 id="batch-title">
                The batch <span class="heading-count">{view.jobs.length}</span>
              </h2>
            </div>
            <button
              class="text-button muted"
              disabled={busy}
              onclick={() => action(() => controller?.clear())}
              >Clear all</button
            >
          </div>
          <div class="job-list">
            {#each view.jobs as job (job.id)}
              {@const result = status(job)}
              <article
                class={["job", `job-${result.tone}`]}
                aria-label={job.name}
              >
                <div class="job-main">
                  <div class="file-symbol">{@render icon("file")}</div>
                  <div class="job-title">
                    <h3 title={job.name}>{job.name}</h3>
                    <p>
                      {job.format
                        ? formatLabel(job.format)
                        : ["queued", "inspecting"].includes(job.status)
                          ? "Detecting format"
                          : "Format not identified"}<span class="meta-separator"
                        >/</span
                      >{bytes(job.size)}{#if job.files.length > 1}<span
                          class="meta-separator">/</span
                        >{job.files.length} files{/if}
                    </p>
                  </div>
                  <button
                    class="icon-button remove-button"
                    aria-label={`Remove ${job.name}`}
                    title="Remove case"
                    disabled={busy}
                    onclick={() => action(() => controller?.remove(job.id))}
                    >{@render icon("close")}</button
                  >
                </div>
                <div class="job-status-line">
                  <span class={["status-pill", result.tone]}
                    ><span class="status-dot"></span>{result.text}</span
                  >{#if job.family}<span class="family-label">{job.family}</span
                    >{/if}
                </div>
                {#if job.status === "needs-primary"}<label class="entry-select"
                    >Which file starts this project?<select
                      disabled={busy}
                      value={job.primary ?? ""}
                      onchange={(event) =>
                        controller?.setPrimary(
                          job.id,
                          event.currentTarget.value,
                        )}
                      ><option value="" disabled>Choose an entry file</option
                      >{#each job.files.filter((path) => path
                          .toLowerCase()
                          .endsWith(".dss")) as path (path)}<option value={path}
                          >{path}</option
                        >{/each}</select
                    ></label
                  >{/if}
                {#if job.status === "error" || job.status === "needs-files"}<p
                    class="job-help"
                  >
                    {#if job.status === "needs-files"}
                      Some referenced files could not be read, so this project
                      is incomplete. Review the details, add missing files, or
                      use the complete project folder. Other cases can still
                      convert.
                    {:else}
                      Other cases can still convert. Review the details, add any
                      missing files, or try a format override below.
                    {/if}
                  </p>
                  <button
                    class="text-button"
                    disabled={busy}
                    onclick={() => chooseMissingFiles(job.id)}
                    >Add missing files</button
                  >{/if}
                {@render diagnostics(job.diagnostics)}
                {#if job.outputs.length}
                  <ul class="output-list">
                    {#each job.outputs as output (output.id)}
                      <li
                        class={[
                          "output",
                          output.status === "warnings" && "output-warning",
                          output.status === "error" && "output-error",
                        ]}
                      >
                        <div class="output-main">
                          <div>
                            <strong>{formatLabel(output.format)}</strong>
                            <p>
                              {output.status === "unchanged"
                                ? "Unchanged, original bytes preserved"
                                : output.status === "warnings"
                                  ? "Converted with warnings"
                                  : output.status === "error"
                                    ? "Couldn't convert to this format"
                                    : "Converted"}{#if output.status !== "error"}<span
                                  class="meta-separator">/</span
                                >{bytes(output.size)}{/if}
                            </p>
                          </div>
                          {#if output.status !== "error"}<button
                              class="button download-button"
                              aria-label={`Download ${job.name} as ${formatLabel(output.format)}`}
                              onclick={() =>
                                action(() =>
                                  controller?.downloadOutput(job.id, output.id),
                                )}
                              >{@render icon("download")}<span>Download</span
                              ></button
                            >{/if}
                        </div>
                        {@render diagnostics(output.diagnostics)}
                      </li>
                    {/each}
                  </ul>
                {/if}
                <div class="job-bottom">
                  <details class="case-settings">
                    <summary>Case settings</summary>
                    <div class="case-setting-fields">
                      <label
                        >Input format<select
                          aria-label="Input format"
                          disabled={busy}
                          value={job.overrideFormat ?? ""}
                          onchange={(event) =>
                            controller?.setFormat(
                              job.id,
                              event.currentTarget.value,
                            )}
                          ><option value="">Detect automatically</option
                          >{#each view.formats.filter((format) => format.canRead) as format (format.token)}<option
                              value={format.token}>{format.label}</option
                            >{/each}</select
                        ></label
                      >{#if job.files.filter((path) => path
                          .toLowerCase()
                          .endsWith(".dss")).length > 1}
                        <label
                          >Entry file<select
                            disabled={busy}
                            value={job.primary ?? ""}
                            onchange={(event) =>
                              controller?.setPrimary(
                                job.id,
                                event.currentTarget.value,
                              )}
                          >
                            <option value="" disabled
                              >Choose an entry file</option
                            >
                            {#each job.files.filter((path) => path
                                .toLowerCase()
                                .endsWith(".dss")) as path (path)}
                              <option value={path}>{path}</option>
                            {/each}
                          </select></label
                        >
                      {/if}{#if job.family}<fieldset>
                          <legend>Output formats for this case</legend><label
                            class="checkbox-label"
                            ><input
                              type="checkbox"
                              disabled={busy}
                              checked={job.targets !== undefined}
                              onchange={(event) =>
                                controller?.setJobTargets(
                                  job.id,
                                  event.currentTarget.checked
                                    ? [...view.targets[job.family!]]
                                    : undefined,
                                )}
                            />Choose different outputs for this case</label
                          >{#if job.targets}<div class="case-targets">
                              {#each view.formats.filter((format) => format.canEmit && format.family === job.family) as format (format.token)}<label
                                  class="checkbox-label"
                                  ><input
                                    type="checkbox"
                                    disabled={busy ||
                                      !formatAvailable(format, job)}
                                    checked={job.targets.includes(format.token)}
                                    onchange={(event) =>
                                      toggleJobTarget(
                                        job,
                                        format.token,
                                        event.currentTarget.checked,
                                      )}
                                  /><span
                                    >{format.label}{#if format.requiresValueType}<small
                                        class="format-requirement"
                                        >{formatRequirement(format)}</small
                                      >{/if}</span
                                  ></label
                                >{/each}
                            </div>{/if}
                        </fieldset>{/if}
                      <details class="source-files">
                        <summary>Included files ({job.files.length})</summary>
                        <ul>
                          {#each job.files as path (path)}<li>{path}</li>{/each}
                        </ul>
                      </details>
                    </div>
                  </details>
                  <div class="job-tools">
                    {#if job.status === "error" || job.status === "cancelled" || job.status === "needs-files"}<button
                        class="text-button"
                        disabled={busy}
                        onclick={() => action(() => controller?.retry(job.id))}
                        >Retry</button
                      >{/if}<button
                      class="text-button muted"
                      onclick={() => openReport(job.id)}>Get help</button
                    >
                  </div>
                </div>
              </article>
            {/each}
          </div>
        </section>
        <aside class="output-settings" aria-labelledby="output-title">
          <div class="section-heading">
            <div>
              <h2 id="output-title">Convert to</h2>
            </div>
          </div>
          <p class="settings-intro">
            Choose one or more formats. Each case uses the formats for its
            network type.
          </p>
          <label class="output-search"
            ><span class="visually-hidden">Find an output format</span><input
              type="search"
              placeholder="Find an output format"
              bind:value={targetSearch}
            /></label
          >
          {#each activeFamilies.filter( (family) => outputFormats.some((format) => format.family === family) ) as family (family)}<fieldset
              class="family-formats"
            >
              <legend
                >{family === "transmission"
                  ? "Transmission"
                  : "Distribution"}</legend
              >{#each outputFormats.filter((format) => format.family === family) as format (format.token)}<label
                  class={[
                    "format-option",
                    view.targets[family].includes(format.token) && "selected",
                    !formatAvailable(format) && "unavailable",
                  ]}
                  ><input
                    type="checkbox"
                    disabled={busy || !formatAvailable(format)}
                    checked={view.targets[family].includes(format.token)}
                    onchange={(event) =>
                      toggleTarget(
                        family,
                        format.token,
                        event.currentTarget.checked,
                      )}
                  /><span
                    >{format.label}{#if format.requiresValueType}<small
                        class="format-requirement"
                        >{formatRequirement(format)}</small
                      >{/if}</span
                  >{#if format.isDirectory && !format.label
                      .toLowerCase()
                      .includes("folder")}<span
                      class="format-kind"
                      aria-hidden="true">folder</span
                    >{/if}</label
                >{/each}
            </fieldset>{/each}
          {#if activeFamilies.length && !outputFormats.some( (format) => activeFamilies.includes(format.family) )}<p
              class="muted-copy"
            >
              No matching output formats.
            </p>{/if}
          {#if !activeFamilies.length}<p class="muted-copy">
              Output choices appear when PowerIO identifies a network.
            </p>{/if}
          {#if activeFamilies.includes("distribution")}<p class="bmopf-note">
              BMOPF is developed by the <a
                href="https://github.com/distribution-system-opt"
                >BMOPF task force</a
              >. The 0.2.0 profile is a proposal.
            </p>{/if}
          <button
            class="text-button share-settings"
            onclick={() => action(() => controller?.share())}
            >{@render icon("share")}Share these settings</button
          >
          <p class="sharing-note">A link to formats, never your files.</p>
        </aside>
      </div>
      <div class="batch-actions">
        <div class="action-summary">
          <strong
            >{outputs.length
              ? `${outputs.length} ${outputs.length === 1 ? "output" : "outputs"} ready`
              : `${plannedCount} ${plannedCount === 1 ? "output" : "outputs"} selected`}</strong
          ><span
            >{failures.length
              ? `${failures.length} ${failures.length === 1 ? "case needs" : "cases need"} attention.${outputs.length ? " Successful outputs are available." : ""}`
              : "Conversion details are included with your batch download."}</span
          >
        </div>
        <div class="action-buttons">
          {#if busy}<button
              class="button secondary"
              onclick={() => controller?.cancel()}>Cancel</button
            >{:else}{#if failures.length}<button
                class="text-button"
                onclick={() => action(() => controller?.retry())}
                >Retry failed</button
              >{/if}{#if outputs.length}<button
                class="text-button report-download"
                onclick={() => action(() => controller?.downloadReport())}
                >Report</button
              ><button
                class="button secondary"
                onclick={() => action(() => controller?.downloadAll())}
                >Download all {@render icon("download")}</button
              >{/if}<button
              class="button primary"
              disabled={!ready.length || !plannedCount}
              onclick={() => action(() => controller?.convert())}
              >{outputs.length ? "Convert again" : "Convert"}
              {@render icon("arrow")}</button
            >{/if}
        </div>
      </div>
    {:else}
      <div class="how-it-works">
        <div>
          <p>
            <strong>Bring your files</strong><span
              >Single cases or whole projects.</span
            >
          </p>
        </div>
        <div>
          <p>
            <strong>Pick your formats</strong><span
              >One output or several. Your choice.</span
            >
          </p>
        </div>
        <div>
          <p>
            <strong>Take them with you</strong><span
              >Download files or the whole batch.</span
            >
          </p>
        </div>
      </div>
    {/if}
  </section>

  <section class="below-fold" aria-label="More about PowerIO Convert">
    <div class="terminal-section">
      <div class="section-heading">
        <div>
          <h2>Meet PowerIO<br />in your terminal.</h2>
        </div>
        <span class="terminal-icon">{@render icon("code")}</span>
      </div>
      <p>
        Automate the next batch, work with larger projects, or build conversion
        into your own tools.
      </p>
      <div class="terminal">
        <div class="terminal-header">
          <span>Terminal</span><button
            onclick={() => {
              controller?.trackCLI();
              return copy(terminalCommands, "Terminal commands copied.");
            }}>Copy commands</button
          >
        </div>
        <pre><code>{terminalCommands}</code></pre>
      </div>
      <div class="language-links">
        <a
          href="https://powerio.dev/guide/getting-started.html"
          onclick={() => controller?.trackCLI()}
          >Get started {@render icon("arrow")}</a
        ><span
          >Also for <a href="https://docs.rs/powerio/">Rust</a>,
          <a href="https://powerio.dev/guide/python.html">Python</a>, and
          <a href="https://github.com/eigenergy/PowerIO.jl">Julia</a>.</span
        >
      </div>
    </div>
    <div class="help-section">
      <details class="info-detail" id="privacy" bind:this={privacyDetails}>
        <summary>Your files stay here.</summary>
        <div>
          <p>
            PowerIO reads and converts files in this browser using WebAssembly.
            File contents, names, and paths are never sent to a server.
          </p>
          <p>
            Closing this tab discards the conversion queue. Completed results
            may remain in temporary browser storage until Clear all removes them
            or PowerIO clears them on your next visit.
          </p>
          <h3 class="analytics-heading">Optional usage statistics</h3>
          <p>
            Usage statistics are off by default. Umami analytics load only if
            you opt in, and only activity after opt-in is shared. Switch them
            off at any time. Do Not Track and Global Privacy Control (GPC) are
            respected.
          </p>
          <p>
            Shared statistics can include standard format names, broad parse or
            conversion outcomes, reviewed parser problem codes, approximate
            batch sizes, broad elapsed-time ranges, and the PowerIO version.
            Unreviewed problem codes are reported as "other".
          </p>
          <p>
            File contents, names, paths, raw error messages, reports, grid
            properties and locations, and electrical diagnostics are never
            included. Connecting to Umami exposes your IP address and browser
            connection metadata. Umami uses this metadata for browser, operating
            system and device information, approximate location (country, region
            and city), and visit statistics. It generates its own session and
            visit IDs. PowerIO does not send a custom visitor ID. <a
              href="https://docs.umami.is/docs/metric-definitions"
              >Umami's data definitions</a
            >
            explain these statistics.
          </p>
          <details class="analytics-example">
            <summary>See an example statistic</summary>
            <dl>
              <dt>Outcome</dt>
              <dd>Parse problem</dd>
              <dt>Format</dt>
              <dd>OpenDSS</dd>
              <dt>Problem code</dt>
              <dd><code>READ.DSS.INCLUDE_LOAD_FAILED</code></dd>
            </dl>
          </details>
          <p>
            <a
              href="https://github.com/eigenergy/powerio/tree/main/apps/converter/public/analytics-policy.js"
              >Review the analytics policy</a
            >
            for the exact allowed fields. Source review and suggestions are welcome
            on GitHub.
          </p>
          <label class="checkbox-label analytics-control"
            ><input
              type="checkbox"
              checked={view.analytics}
              onchange={(event) => {
                const input = event.currentTarget;
                controller?.setAnalytics(input.checked);
                input.checked = controller?.state.analytics ?? false;
              }}
            />Share limited usage and error statistics</label
          >
        </div>
      </details>
      <details class="info-detail">
        <summary>What does a warning mean?</summary>
        <div>
          <p>
            Formats do not all describe a grid in the same way. PowerIO reports
            data that a target format cannot represent, rather than silently
            leaving it out.
          </p>
          <p>
            A converted file can still have warnings. Review them before using
            the output. Original source bytes are preserved when writing the
            same format.
          </p>
          <a href="https://powerio.dev/guide/format-fidelity.html"
            >Understand conversion fidelity</a
          >
        </div>
      </details>
      <details class="info-detail">
        <summary>Working with a large project?</summary>
        <div>
          <p>
            Each project can contain up to 4,096 files and 64 MiB of expanded
            data. There is no fixed limit on the number of projects in a batch.
            Available browser memory and storage also apply.
          </p>
          <p>
            For a project that exceeds those limits, use the same PowerIO
            converter in your terminal. Folder-based inputs need their related
            files, so choose the complete project folder or ZIP.
          </p>
        </div>
      </details>
      <div class="help-callout">
        <h3>Not the conversion you expected?</h3>
        <p>
          A small report helps make PowerIO better for everyone. Review it
          before opening a GitHub issue.
        </p>
        <button class="text-button" onclick={() => openReport()}
          >Report a conversion problem {@render icon("arrow")}</button
        >
      </div>
    </div>
  </section>

  <section id="formats" class="format-catalog" aria-labelledby="format-title">
    <div class="catalog-heading">
      <div>
        <h2 id="format-title">Supported formats</h2>
      </div>
      <label class="format-search"
        ><span class="visually-hidden">Find a format</span><input
          type="search"
          placeholder="Find a format"
          bind:value={formatSearch}
        /></label
      >
    </div>
    <p>
      Transmission and distribution formats, with the same parsers as the
      PowerIO package.
    </p>
    <div class="format-table-wrap">
      <table>
        <thead
          ><tr
            ><th scope="col">Format</th><th scope="col">Network</th><th
              scope="col">Read</th
            ><th scope="col">Write</th></tr
          ></thead
        ><tbody
          >{#each visibleFormats as format (format.token)}<tr
              ><th scope="row"
                >{#if format.token.startsWith("bmopf")}<a
                    href="https://github.com/distribution-system-opt"
                    >{format.label}</a
                  >{:else}{format.label}{/if}{#if format.isDirectory}<span
                    class="format-table-note">folder</span
                  >{/if}</th
              ><td class="catalog-family">{format.family}</td><td
                >{format.canRead ? "Yes" : "No"}</td
              ><td
                >{format.canEmit
                  ? format.requiresValueType
                    ? "Conditional"
                    : "Yes"
                  : "Read only"}{#if format.canEmit && format.requiresValueType}<small
                    class="format-requirement"
                    >{formatRequirement(format)}</small
                  >{/if}</td
              ></tr
            >{/each}{#if !visibleFormats.length}<tr
              ><td colspan="4"
                >{view.phase === "loading"
                  ? "Loading format information..."
                  : "No matching formats."}</td
              ></tr
            >{/if}</tbody
        >
      </table>
    </div>
  </section>
</main>

<footer class="site-footer">
  <a class="brand" href="https://powerio.dev/"
    ><img src={`${base}powerio-logo.svg`} alt="" width="28" height="28" /><span
      >PowerIO</span
    ></a
  >
  <p>Open source. Built for the power systems community.</p>
  <div class="footer-links">
    <a
      href="#privacy"
      onclick={() => {
        privacyDetails.open = true;
      }}>Privacy</a
    ><a href="https://github.com/eigenergy/powerio">Source code</a
    >{#if view.engine.version}<span
        >Engine {view.engine.version}{#if view.engine.commit}<a
            href={`https://github.com/eigenergy/powerio/commit/${view.engine.commit}`}
            class="commit-link">{view.engine.commit.slice(0, 7)}</a
          >{/if}</span
      >{/if}
  </div>
</footer>

<dialog
  class="report-dialog"
  bind:this={reportDialog}
  aria-labelledby="report-title"
>
  <div class="dialog-heading">
    <div>
      <h2 id="report-title">Report a conversion problem</h2>
    </div>
    <button
      class="icon-button"
      aria-label="Close report"
      onclick={() => reportDialog.close()}>{@render icon("close")}</button
    >
  </div>
  <p>
    Tell us what you expected and what happened. This report starts with formats
    and diagnostic codes; filenames and file contents are left out.
  </p>
  <label for="issue-report">Review and edit your report</label><textarea
    id="issue-report"
    bind:value={report}
    rows="12"
    spellcheck="false"></textarea>
  <p class="public-notice">
    GitHub issues are public. Review the report before sharing. Files are never
    attached automatically.
  </p>
  <div class="dialog-actions">
    <button
      class="button secondary"
      onclick={() => copy(report, "Issue report copied.")}>Copy report</button
    ><a
      class="button primary"
      href={reportUrl()}
      target="_blank"
      rel="noopener noreferrer">Open GitHub issue {@render icon("arrow")}</a
    >
  </div>
  {#if notice}<p role="status">{notice}</p>{/if}
</dialog>
