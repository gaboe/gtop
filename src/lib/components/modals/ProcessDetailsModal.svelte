<script lang="ts">
  import { Modal } from "$lib/components";
  import { formatBytes } from "$lib/utils";
  import { invoke } from "@tauri-apps/api/core";
  import type { Process } from "$lib/types";
  import Fa from "svelte-fa";
  import {
    faMemory,
    faMicrochip,
    faCodeFork,
    faTerminal,
    faList,
  } from "@fortawesome/free-solid-svg-icons";

  export let show = false;
  export let process: Process | null = null;
  export let onClose: () => void;
  export let processes: Process[] = [];
  export let onShowDetails: (process: Process) => void;

  let environ: string[] = [];
  let environPid: number | null = null;

  async function loadEnviron(pid: number) {
    environPid = pid;
    environ = [];
    const result = await invoke<string[]>("get_process_environ", { pid });
    if (environPid === pid) environ = result;
  }

  $: if (!show) environPid = null;
  $: if (show && process && process.pid !== environPid)
    loadEnviron(process.pid);

  $: childProcesses = process
    ? processes.filter((p) => p.ppid === process.pid)
    : [];
</script>

<Modal
  {show}
  title={`${process ? process.name.slice(0, 10) : "Unknown Process"} - Process Details`}
  maxWidth="1000px"
  {onClose}
>
  {#if process}
    <div class="modal-content">
      <!-- Header Stats -->
      <div class="header-stats">
        <div class="stat-item">
          <div class="stat-label">PID</div>
          <div class="stat-value">{process.pid}</div>
        </div>
        <div class="stat-item">
          <div class="stat-label">Status</div>
          <div
            class="stat-value status"
            class:running={process.status === "Running"}
          >
            {process.status}
          </div>
        </div>
        <div class="stat-item">
          <div class="stat-label">CPU</div>
          <div class="stat-value">{process.cpu_usage.toFixed(1)}%</div>
        </div>
        <div class="stat-item">
          <div class="stat-label">Memory</div>
          <div class="stat-value">{formatBytes(process.memory_usage)}</div>
        </div>
      </div>

      <!-- Main Content -->
      <div class="content-grid">
        <!-- Left Column -->
        <div class="content-column">
          <!-- Process Info -->
          <div class="card">
            <div class="card-header">
              <Fa icon={faMicrochip} />
              <span>Process Information</span>
            </div>
            <div class="card-content">
              <div class="info-grid">
                <div class="info-item">
                  <span class="info-label">Name</span>
                  <span class="info-value">{process.name}</span>
                </div>
                <div class="info-item">
                  <span class="info-label">User</span>
                  <span class="info-value">{process.user}</span>
                </div>
                <div class="info-item">
                  <span class="info-label">Parent PID</span>
                  <!-- svelte-ignore a11y_click_events_have_key_events -->
                  <!-- svelte-ignore a11y_no_static_element_interactions -->
                  <span
                    class="info-value clickable"
                    on:click={() => {
                      const parent = processes.find(
                        (p) => p.pid === process.ppid,
                      );
                      if (parent) onShowDetails(parent);
                    }}
                  >
                    {process.ppid}
                  </span>
                </div>
                <div class="info-item">
                  <span class="info-label">Session ID</span>
                  <span class="info-value">{process.session_id}</span>
                </div>
              </div>
            </div>
          </div>

          <!-- Resource Usage -->
          <div class="card">
            <div class="card-header">
              <Fa icon={faMemory} />
              <span>Resource Usage</span>
            </div>
            <div class="card-content">
              <div class="resource-grid">
                <div class="resource-item">
                  <div class="resource-header">
                    <span>CPU Usage</span>
                    <span class="resource-value"
                      >{process.cpu_usage.toFixed(1)}%</span
                    >
                  </div>
                  <div class="progress-bar">
                    <div
                      class="progress-fill"
                      style="width: {process.cpu_usage}%"
                      class:high={process.cpu_usage > 50}
                      class:critical={process.cpu_usage > 80}
                    ></div>
                  </div>
                </div>
                <div class="resource-item">
                  <div class="resource-header">
                    <span>Memory Usage</span>
                  </div>
                  <div class="memory-stats">
                    <div>Physical: {formatBytes(process.memory_usage)}</div>
                    <div>Virtual: {formatBytes(process.virtual_memory)}</div>
                  </div>
                </div>
                <div class="resource-item">
                  <div class="resource-header">
                    <span>Disk I/O</span>
                  </div>
                  <div class="disk-stats">
                    <div>Read: {formatBytes(process.disk_usage[0])}</div>
                    <div>Written: {formatBytes(process.disk_usage[1])}</div>
                  </div>
                </div>
              </div>
            </div>
          </div>
        </div>

        <!-- Right Column -->
        <div class="content-column">
          <!-- Command -->
          <div class="card">
            <div class="card-header">
              <Fa icon={faTerminal} />
              <span>Command</span>
            </div>
            <div class="card-content">
              <div class="command-text">{process.command}</div>
              <div class="path-text">{process.root}</div>
            </div>
          </div>

          <!-- Child Processes -->
          {#if childProcesses.length > 0}
            <div class="card">
              <div class="card-header">
                <Fa icon={faCodeFork} />
                <span>Child Processes ({childProcesses.length})</span>
              </div>
              <div class="card-content">
                <table class="process-table">
                  <thead>
                    <tr>
                      <th>Name</th>
                      <th>PID</th>
                      <th>CPU</th>
                      <th>Memory</th>
                    </tr>
                  </thead>
                  <tbody>
                    {#each childProcesses as child}
                      <tr
                        class="clickable"
                        on:click={() => onShowDetails(child)}
                      >
                        <td>{child.name}</td>
                        <td>{child.pid}</td>
                        <td>{child.cpu_usage.toFixed(1)}%</td>
                        <td>{formatBytes(child.memory_usage)}</td>
                      </tr>
                    {/each}
                  </tbody>
                </table>
              </div>
            </div>
          {/if}

          <!-- Environment Variables -->
          {#if environ.length > 0}
            <div class="card">
              <div class="card-header">
                <Fa icon={faList} />
                <span>Environment Variables</span>
              </div>
              <div class="card-content">
                <div class="env-list">
                  {#each environ as env}
                    <div class="env-item">{env}</div>
                  {/each}
                </div>
              </div>
            </div>
          {/if}
        </div>
      </div>
    </div>
  {/if}
</Modal>

<style>
  /* Base Modal Content */
  .modal-content {
    display: flex;
    flex-direction: column;
    gap: 24px;
    font-size: 13px;
    color: var(--text);
  }

  /* Header Stats */
  .header-stats {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(120px, 1fr));
    gap: 16px;
    padding: 16px;
    background: var(--surface0);
    border-radius: 8px;
  }

  .stat-item {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .stat-label {
    font-size: 12px;
    color: var(--subtext0);
    font-weight: 500;
  }

  .stat-value {
    font-size: 16px;
    font-weight: 600;
  }

  .stat-value.status {
    color: var(--subtext0);
  }

  .stat-value.status.running {
    color: var(--green);
  }

  /* Main Content Grid */
  .content-grid {
    display: grid;
    grid-template-columns: minmax(300px, 0.4fr) minmax(400px, 0.6fr);
    gap: 24px;
  }

  .content-column {
    display: flex;
    flex-direction: column;
    gap: 24px;
    min-width: 0; /* Prevent overflow issues */
  }

  /* Cards */
  .card {
    background: var(--surface0);
    border-radius: 8px;
    overflow: hidden;
    min-width: 0; /* Prevent overflow issues */
  }

  .card-header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 12px 16px;
    background: var(--surface1);
    color: var(--subtext0);
    font-weight: 500;
  }

  .card-header :global(svg) {
    width: 14px;
    height: 14px;
    color: var(--blue);
  }

  .card-content {
    padding: 16px;
    overflow: auto;
  }

  /* Info Grid */
  .info-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
    gap: 16px;
  }

  .info-item {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .info-label {
    color: var(--subtext0);
    font-size: 12px;
  }

  .info-value {
    color: var(--text);
  }

  .info-value.clickable {
    cursor: pointer;
    color: var(--blue);
  }

  .info-value.clickable:hover {
    text-decoration: underline;
  }

  /* Resource Usage */
  .resource-grid {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .resource-item {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .resource-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    color: var(--subtext0);
    font-size: 12px;
  }

  .resource-value {
    color: var(--text);
  }

  /* Progress Bar */
  .progress-bar {
    height: 6px;
    background: var(--surface1);
    border-radius: 3px;
    overflow: hidden;
  }

  .progress-fill {
    height: 100%;
    background: var(--blue);
    transition: width 0.2s ease;
  }

  .progress-fill.high {
    background: var(--yellow);
  }

  .progress-fill.critical {
    background: var(--red);
  }

  /* Memory and Disk Stats */
  .memory-stats,
  .disk-stats {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 8px;
    color: var(--text);
  }

  /* Command and Path */
  .command-text,
  .path-text {
    word-break: break-all;
    white-space: pre-wrap;
  }

  .path-text {
    margin-top: 8px;
    font-size: 12px;
    color: var(--subtext0);
  }

  /* Process Table */
  .process-table {
    width: 100%;
    border-collapse: collapse;
  }

  .process-table th {
    text-align: left;
    padding: 8px;
    color: var(--subtext0);
    font-weight: 500;
    border-bottom: 1px solid var(--surface1);
  }

  .process-table td {
    padding: 8px;
    border-bottom: 1px solid var(--surface1);
  }

  .process-table tr:last-child td {
    border-bottom: none;
  }

  .process-table tr.clickable {
    cursor: pointer;
  }

  .process-table tr.clickable:hover {
    background: var(--surface1);
  }

  /* Environment Variables */
  .env-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
    max-height: 200px;
    overflow-y: auto;
    margin: -16px;
    padding: 16px;
  }

  .env-item {
    font-family:
      ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono",
      "Courier New", monospace;
    padding: 4px 8px;
    border-radius: 4px;
    color: var(--subtext1);
    font-size: 12px;
  }

  .env-item:hover {
    background: var(--surface1);
  }

  /* Update scrollbar styles to match the container edges */
  .env-list::-webkit-scrollbar {
    width: 8px;
    height: 8px;
  }

  .env-list::-webkit-scrollbar-track {
    background: var(--surface0);
    border-radius: 0;
  }

  .env-list::-webkit-scrollbar-thumb {
    background: var(--surface2);
    border-radius: 4px;
    border: 2px solid var(--surface0);
  }

  .env-list::-webkit-scrollbar-thumb:hover {
    background: var(--surface1);
  }

  /* Scrollbar Styles */
  :global(.modal-content *::-webkit-scrollbar) {
    width: 8px;
    height: 8px;
  }

  :global(.modal-content *::-webkit-scrollbar-track) {
    background: var(--mantle);
    border-radius: 4px;
  }

  :global(.modal-content *::-webkit-scrollbar-thumb) {
    background: var(--surface2);
    border-radius: 4px;
  }

  :global(.modal-content *::-webkit-scrollbar-thumb:hover) {
    background: var(--surface1);
  }

  /* Responsive Design */
  @media (max-width: 900px) {
    .content-grid {
      grid-template-columns: 1fr;
    }

    .header-stats {
      grid-template-columns: repeat(2, 1fr);
    }
  }
</style>
