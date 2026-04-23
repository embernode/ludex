// Centralised ECharts module registration.
//
// ECharts 6 is modular: importing from `echarts/core` pulls in just
// the runtime, and each chart/component has to be registered
// explicitly via `echarts.use(...)`. Keeping that registration in
// one place means every chart in the app ends up with the same set
// of capabilities and the bundle doesn't pull in anything it won't
// render.

import * as echarts from 'echarts/core';
import { BarChart, HeatmapChart, LineChart } from 'echarts/charts';
import {
    CalendarComponent,
    GridComponent,
    TitleComponent,
    TooltipComponent,
    VisualMapComponent,
} from 'echarts/components';
import { CanvasRenderer } from 'echarts/renderers';

echarts.use([
    BarChart,
    HeatmapChart,
    LineChart,
    CalendarComponent,
    GridComponent,
    TitleComponent,
    TooltipComponent,
    VisualMapComponent,
    CanvasRenderer,
]);

export { echarts };
export type { EChartsCoreOption } from 'echarts/core';
