<script setup lang="ts">
import { ZCard } from '@codex-proxy/ui'

import { usageRecordColumns } from '@/components/usage/shared/columns'
import { dashboardSnapshotView } from '@/components/usage/shared/dashboard'
import UsageRecordsTable from '@/components/usage/UsageRecordsTable.vue'
import DashboardContent from '@/views/dashboard/components/DashboardContent.vue'

import { themeDashboardSummary } from '../fixtures/dashboard'

const snapshot = dashboardSnapshotView(themeDashboardSummary)
const previewColumns = usageRecordColumns.filter(column => column.key !== 'actions')
</script>

<template>
  <DashboardContent
    :analytics="snapshot.analytics"
    :health-timeline="snapshot.healthTimeline"
    :account-usage="snapshot.accountUsage"
    :wire-profiles="snapshot.wireProfiles"
    :pool-summary="snapshot.poolSummary"
    :capacity-info="snapshot.capacityInfo"
    :rotation-strategy="snapshot.rotationStrategy"
  >
    <template #requests>
      <ZCard title="请求明细" description="查看请求结果、用量与错误详情" class="mt-6">
        <template #body>
          <UsageRecordsTable :columns="previewColumns" :rows="themeDashboardSummary.usageRecords" />
        </template>
      </ZCard>
    </template>
  </DashboardContent>
</template>
