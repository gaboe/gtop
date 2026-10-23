<script lang="ts">
  import { faTemperatureHalf } from "@fortawesome/free-solid-svg-icons";
  import { PanelHeader, StatItem } from "$lib/components";
  import type { Temperature } from "$lib/types";

  export let temperatures: Temperature[];
</script>

<div class="stat-panel">
  <PanelHeader icon={faTemperatureHalf} title="Temps" />
  <div class="temperature-grid">
    {#each temperatures as temperature (temperature.name)}
      <StatItem
        label={temperature.name}
        value={`${temperature.celsius.toFixed(0)}°C`}
      />
    {/each}
  </div>
</div>

<style>
  .stat-panel {
    flex: 0.8;
    min-width: 110px;
    background-color: var(--mantle);
    border-radius: 6px;
    padding: 0.75rem;
    display: flex;
    flex-direction: column;
  }

  .temperature-grid {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    flex: 1;
  }
</style>
