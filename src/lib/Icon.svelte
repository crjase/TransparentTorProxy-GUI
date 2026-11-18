<script lang="ts">
  let { name, spinning = false }: {
    name: 'gear' | 'rotate' | 'check' | 'hide' | 'quit' | 'close' | 'route'
    spinning?: boolean
  } = $props()
</script>

<svg class:spinning width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
  {#if name === 'gear'}
    <path d="m9.7 3 .7-1h3.2l.6 2.2 1.8 1 2.2-.7 1.7 2.8-1.6 1.6.2 2.1 1.9 1.3-.8 3.1-2.3.1-1.4 1.6.1 2.3-3 1.1-1.5-1.8-2.1-.4-1.9 1.3-2.6-1.9.7-2.2-.9-1.9-2.2-.7V9.8l2.2-.8 1-1.9-.6-2.2 2.8-1.7 1.7 1.3z" transform="translate(.7 .8) scale(.94)" />
    <circle cx="12" cy="12" r="3.1"/>
  {:else if name === 'rotate'}
    <path d="M20 8a8.3 8.3 0 1 0 .2 7M20 3.8v4.8h-4.8"/>
  {:else if name === 'check'}
    <circle cx="12" cy="12" r="8.7"/><path d="m8.1 12 2.6 2.5 5.1-5.2"/>
  {:else if name === 'hide'}
    <rect x="3.2" y="4.5" width="17.6" height="15" rx="3"/><path d="M4.8 14.7h14.4m-8.9 2.3h3.4"/>
  {:else if name === 'quit'}
    <path d="M10 4H5.5A1.5 1.5 0 0 0 4 5.5v13A1.5 1.5 0 0 0 5.5 20H10m-1-8h12m-4.5-4.5L21 12l-4.5 4.5"/>
  {:else if name === 'close'}
    <path d="m6.5 6.5 11 11m0-11-11 11"/>
  {:else}
    <path d="M3 17h5.5l7-10H21m-4-4 4 4-4 4"/>
  {/if}
</svg>

<style>
  .spinning { animation: turn 460ms ease-in-out infinite; }
  @keyframes turn { to { transform: rotate(360deg); } }
  @media (prefers-reduced-motion: reduce) { .spinning { animation: none; } }
</style>
