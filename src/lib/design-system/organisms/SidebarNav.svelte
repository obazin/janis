<script lang="ts">
    import NavItem from '../molecules/NavItem.svelte';
    import SectionLabel from '../atoms/SectionLabel.svelte';

    // Content-agnostic sidebar: labelled sections of nav rows + a sticky
    // footer row. Caller supplies pre-translated labels, path data, the
    // active id and the click handler — the organism touches no stores and
    // no i18n.
    export interface NavEntry {
        id: string;
        label: string;
        d: string;
    }

    interface Props {
        sections: { label: string; items: NavEntry[] }[];
        footerItem: NavEntry;
        activeId: string;
        onItemClick: (id: string) => void;
    }

    let { sections, footerItem, activeId, onItemClick }: Props = $props();
</script>

<nav
    class="w-58 flex-none px-3.5 py-5 flex flex-col gap-1.5 border-r border-divider overflow-y-auto mscroll"
>
    {#each sections as section, i (section.label)}
        <SectionLabel class="px-3 {i === 0 ? 'pb-1.5' : 'pt-3.5 pb-1.5'}">
            {section.label}
        </SectionLabel>
        {#each section.items as item (item.id)}
            <NavItem
                label={item.label}
                d={item.d}
                active={activeId === item.id}
                onclick={() => onItemClick(item.id)}
            />
        {/each}
    {/each}
    <div class="flex-1"></div>
    <NavItem
        label={footerItem.label}
        d={footerItem.d}
        active={activeId === footerItem.id}
        onclick={() => onItemClick(footerItem.id)}
    />
</nav>
