// The expiry test reads the time zone, so it is set before anything formats a date.
process.env.TZ = 'Asia/Ho_Chi_Minh';

import { render, screen, within } from '@testing-library/vue';
import userEvent from '@testing-library/user-event';
import { createPinia, setActivePinia } from 'pinia';
import { nextTick } from 'vue';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createMemoryHistory } from 'vue-router';

import * as api from '@/api';
import { ApiError, type DroppedElement, type JobErrorKind, type JobStatus, type ServerInfo } from '@/api';
import App from '@/App.vue';
import { createAppI18n } from '@/app/i18n';
import { createAppRouter } from '@/app/router';
import en from '@/features/convert/locales/en.json';
import viConvert from '@/features/convert/locales/vi.json';
import { basicFileInfo, fileOf, makeOle } from '@/shared/revit-file/makeOle.testutil';

import { JOB_STORAGE_KEY, POLL_INTERVAL_MS } from '@/features/convert/stores/convertStore';

vi.mock('@/api', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/api')>();
  return {
    ...actual,
    getInfo: vi.fn(),
    createJob: vi.fn(),
    getJob: vi.fn(),
    getDropped: vi.fn(),
    deleteJob: vi.fn(),
    sendReport: vi.fn(),
    uploadFile: vi.fn(),
  };
});

const mocked = vi.mocked(api);
const JOB = '0123456789abcdef0123456789abcdef';
const INFO: ServerInfo = {
  maxBytes: 1_073_741_824,
  outputTtlS: 3600,
  pairs: [
    [2026, 2025],
    [2026, 2024],
  ],
  version: '0.1.0',
};

function status(overrides: Partial<JobStatus>): JobStatus {
  return {
    state: 'queued',
    position: null,
    elapsedS: 0,
    estimateS: 0,
    expiresAt: null,
    error: null,
    result: null,
    reportCode: null,
    name: 'tower.rvt',
    size: 1000,
    kind: 'rvt',
    year: 2026,
    target: 2025,
    ...overrides,
  };
}

const DONE = status({
  state: 'done',
  expiresAt: '2026-10-25T08:40:00Z',
  result: { fileName: 'tower_R2025.rvt', size: 133 * 1024 * 1024, droppedCount: 4 },
});

const DROPPED: DroppedElement[] = [
  { elementId: 501, class: 'Wall', reason: 'no_rule', cascadeFrom: null },
  { elementId: 502, class: 'Wall', reason: 'no_rule', cascadeFrom: null },
  { elementId: 503, class: 'Door', reason: 'rule_broken', cascadeFrom: null },
  { elementId: 504, class: 'Tag', reason: 'cascade', cascadeFrom: 501 },
];

function revitFile(year = 2026, name = 'tower.rvt'): File {
  return fileOf(makeOle([['BasicFileInfo', basicFileInfo(year)]]), name);
}

/** Lets promise callbacks and the timers they schedule run, with the clock frozen. */
async function settle() {
  for (let round = 0; round < 6; round += 1) {
    await vi.advanceTimersByTimeAsync(0);
    await nextTick();
  }
}

async function open(path = '/', locale = 'en-US') {
  const router = createAppRouter(createMemoryHistory());
  await router.push(path);
  await router.isReady();
  const i18n = createAppI18n(locale, { getItem: () => null, setItem: () => undefined });
  render(App, { global: { plugins: [createPinia(), router, i18n] } });
  await settle();
  return { router, i18n };
}

const user = () => userEvent.setup({ delay: null });

async function failedJob(kind: JobErrorKind = 'failed', message = 'x') {
  mocked.getJob.mockResolvedValue(status({ state: 'failed', error: { kind, message } }));
  return open(`/job/${JOB}`);
}

beforeEach(() => {
  setActivePinia(createPinia());
  localStorage.clear();
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  mocked.getInfo.mockResolvedValue(INFO);
  mocked.createJob.mockResolvedValue({ jobId: JOB, uploadUrl: `/api/uploads/${JOB}` });
  mocked.uploadFile.mockResolvedValue(undefined);
  mocked.deleteJob.mockResolvedValue(undefined);
  mocked.getDropped.mockResolvedValue(DROPPED);
});

afterEach(() => {
  vi.useRealTimers();
});

describe('conversion card', () => {
  it('card_walks_pick_ready_running_done', async () => {
    mocked.getJob
      .mockResolvedValueOnce(status({ state: 'queued', position: 2 }))
      .mockResolvedValueOnce(status({ state: 'running', elapsedS: 65, estimateS: 240 }))
      .mockResolvedValue(DONE);
    const { router } = await open();
    expect(screen.getByRole('button', { name: 'Add a Revit file' })).toBeInTheDocument();

    const file = revitFile();
    await user().upload(screen.getByLabelText('Revit file'), file);
    await settle();
    expect(screen.getByText(/^Revit 2026 · Project · /)).toBeInTheDocument();
    expect(screen.getByText('tower.rvt')).toBeInTheDocument();
    expect(screen.getByRole('combobox', { name: 'Target Revit version' })).toHaveValue('2025');
    expect(
      screen.getByText('Your file is only sent when you press Downgrade. It is deleted after 1 hour.'),
    ).toBeInTheDocument();
    expect(mocked.createJob).not.toHaveBeenCalled();

    await user().click(screen.getByRole('button', { name: 'Downgrade' }));
    await settle();
    expect(mocked.createJob).toHaveBeenCalledWith(expect.objectContaining({ year: 2026, target: 2025, kind: 'rvt' }));
    expect(router.currentRoute.value.name).toBe('job');
    expect(router.currentRoute.value.params.id).toBe(JOB);
    expect(screen.getByText('Waiting — 2 files ahead')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Cancel and delete file' })).toBeInTheDocument();

    await vi.advanceTimersByTimeAsync(POLL_INTERVAL_MS);
    await settle();
    expect(screen.getByText('Converting 2026 → 2025 — 1:05 / about 4:00')).toBeInTheDocument();
    const bar = screen.getByRole('progressbar');
    expect(bar).toHaveAttribute('aria-valuenow', '27');

    await vi.advanceTimersByTimeAsync(POLL_INTERVAL_MS);
    await settle();
    const download = screen.getByRole('link', { name: 'Download tower_R2025.rvt (133 MB)' });
    expect(download).toHaveAttribute('href', api.jobFileUrl(JOB));
    expect(download).toHaveAttribute('download', 'tower_R2025.rvt');
    expect(screen.getByRole('button', { name: 'Delete now' })).toBeInTheDocument();

    await user().click(screen.getByRole('button', { name: 'Convert another file' }));
    await settle();
    expect(screen.getByRole('button', { name: 'Add a Revit file' })).toBeInTheDocument();
    expect(router.currentRoute.value.name).toBe('convert');
    expect(localStorage.getItem(JOB_STORAGE_KEY)).toBeNull();
  });

  it('shows the upload progress as a progressbar with a percentage', async () => {
    mocked.uploadFile.mockImplementation((_file, _url, options) => {
      options?.onProgress?.(25, 100);
      return new Promise(() => undefined);
    });
    await open();
    await user().upload(screen.getByLabelText('Revit file'), revitFile());
    await settle();
    await user().click(screen.getByRole('button', { name: 'Downgrade' }));
    await settle();
    expect(screen.getByText('Uploading... 25%')).toBeInTheDocument();
    expect(screen.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '25');
  });

  it('chooses another file from the ready step', async () => {
    await open();
    await user().upload(screen.getByLabelText('Revit file'), revitFile());
    await settle();
    await user().click(screen.getByRole('button', { name: 'Choose another file' }));
    await settle();
    expect(screen.getByRole('button', { name: 'Add a Revit file' })).toBeInTheDocument();
  });

  it('resume_button_after_retries_exhausted', async () => {
    mocked.uploadFile.mockRejectedValueOnce(new ApiError(0, 'upload_failed', 'offline'));
    // The server still waits for the data.
    mocked.getJob.mockResolvedValue(status({ state: 'waiting_upload' }));
    await open();
    await user().upload(screen.getByLabelText('Revit file'), revitFile());
    await settle();
    await user().click(screen.getByRole('button', { name: 'Downgrade' }));
    await settle();
    expect(screen.getByRole('button', { name: 'Resume' })).toBeInTheDocument();
    expect(screen.getByText(en.convert.progress.interrupted)).toBeInTheDocument();

    mocked.getJob.mockResolvedValue(status({ state: 'queued', position: 0 }));
    await user().click(screen.getByRole('button', { name: 'Resume' }));
    await settle();
    expect(mocked.uploadFile).toHaveBeenCalledTimes(2);
    expect(mocked.uploadFile.mock.calls[1]?.[1]).toBe(`/api/uploads/${JOB}`);
    expect(screen.queryByRole('button', { name: 'Resume' })).not.toBeInTheDocument();
    expect(screen.getByText('Waiting — you are next')).toBeInTheDocument();
  });

  it('retry_after_message_for_429', async () => {
    mocked.createJob.mockRejectedValue(new ApiError(429, 'hourly_limit', 'slow down', 1800));
    await open();
    await user().upload(screen.getByLabelText('Revit file'), revitFile());
    await settle();
    await user().click(screen.getByRole('button', { name: 'Downgrade' }));
    await settle();
    expect(screen.getByRole('alert')).toHaveTextContent('Too many conversions from your network. Try again in 30 min.');
    expect(screen.getByRole('button', { name: 'Choose another file' })).toBeInTheDocument();
  });

  it('explains a file the browser cannot accept, before anything is sent', async () => {
    await open();
    await user().upload(screen.getByLabelText('Revit file'), revitFile(2022));
    await settle();
    expect(screen.getByRole('alert')).toHaveTextContent('Revit 2022 files cannot be downgraded yet. Nothing was sent.');
    expect(mocked.createJob).not.toHaveBeenCalled();
  });

  it('announces the step in a polite live region', async () => {
    await open();
    const region = screen.getByRole('status');
    expect(region).toHaveAttribute('aria-live', 'polite');
    expect(region).toHaveTextContent('Choose a file');
  });
});

describe('finished job', () => {
  it('expiry_shown_in_local_time', async () => {
    mocked.getJob.mockResolvedValue(DONE);
    await open(`/job/${JOB}`);
    expect(screen.getByText('Deleted automatically at 15:40.')).toBeInTheDocument();
  });

  it('dropped_grouped_and_ids_expand', async () => {
    mocked.getJob.mockResolvedValue(DONE);
    await open(`/job/${JOB}`);
    expect(screen.getByText('4 elements were removed')).toBeInTheDocument();
    const rows = screen.getAllByRole('row').slice(1);
    expect(rows.map((row) => row.textContent)).toEqual([
      'WallNot supported yet2',
      'DoorCould not be converted1',
      'TagDepends on a removed element1',
    ]);

    const toggle = screen.getByRole('button', { name: 'Show element IDs' });
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    expect(screen.queryByText(/^501 ·/)).not.toBeVisible();
    await user().click(toggle);
    expect(screen.getByRole('button', { name: 'Hide element IDs' })).toHaveAttribute('aria-expanded', 'true');
    const list = screen.getByRole('list');
    expect(within(list).getByText('501 · Wall · Not supported yet')).toBeVisible();
    expect(within(list).getByText('504 · Tag · Depends on removed element 501')).toBeVisible();
  });

  it('says so when the removed-elements list could not be loaded', async () => {
    mocked.getJob.mockResolvedValue(DONE);
    mocked.getDropped.mockRejectedValue(new ApiError(0, 'network', 'offline'));
    await open(`/job/${JOB}`);
    expect(screen.getByText(en.convert.dropped.loadFailed)).toBeInTheDocument();
    expect(screen.getByRole('link', { name: /^Download tower_R2025\.rvt/ })).toBeInTheDocument();
  });

  it('delete now removes the job and goes back to the start', async () => {
    mocked.getJob.mockResolvedValue(DONE);
    const { router } = await open(`/job/${JOB}`);
    await user().click(screen.getByRole('button', { name: 'Delete now' }));
    await settle();
    expect(mocked.deleteJob).toHaveBeenCalledWith(JOB);
    expect(screen.getByRole('button', { name: 'Add a Revit file' })).toBeInTheDocument();
    expect(router.currentRoute.value.name).toBe('convert');
  });
});

describe('failure report', () => {
  it('consent_unchecked_by_default', async () => {
    await failedJob();
    const consent = screen.getByRole('checkbox');
    expect(consent).not.toBeChecked();
    expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled();
    expect(screen.getByLabelText(/^Let Backsave keep this file for up to 30 days to find and fix the problem\./)).toBe(
      consent,
    );
  });

  it('report_sent_only_after_tick_and_send', async () => {
    mocked.sendReport.mockResolvedValue('R-ABC234');
    await failedJob();
    await user().click(screen.getByRole('button', { name: 'Send' }));
    expect(mocked.sendReport).not.toHaveBeenCalled();

    await user().click(screen.getByRole('checkbox'));
    expect(mocked.sendReport).not.toHaveBeenCalled();
    await user().click(screen.getByRole('button', { name: 'Send' }));
    await settle();
    expect(mocked.sendReport).toHaveBeenCalledExactlyOnceWith(JOB);
    expect(screen.getByText(/R-ABC234/)).toBeInTheDocument();
    expect(screen.queryByRole('checkbox')).not.toBeInTheDocument();
  });

  it('shows why the report could not be sent', async () => {
    mocked.sendReport.mockRejectedValue(new ApiError(0, 'network', 'offline'));
    await failedJob();
    await user().click(screen.getByRole('checkbox'));
    await user().click(screen.getByRole('button', { name: 'Send' }));
    await settle();
    expect(screen.getByText('The report could not be sent (network). Please try again.')).toBeInTheDocument();
    expect(screen.getByRole('checkbox')).toBeChecked();
  });

  it.each<[JobErrorKind, string]>([
    ['refused', 'Not supported yet: no rule for Foo'],
    ['corrupt', 'The file could not be read. It may be damaged.'],
    ['failed', 'The conversion failed. The problem has been logged.'],
    ['crashed', 'The conversion failed. The problem has been logged.'],
    ['timed_out', 'The conversion failed. The problem has been logged.'],
    ['mismatch', 'The file is not what the browser reported. Please try again.'],
    ['disk_full', 'The server is out of space. Please try again later.'],
  ])('each_error_kind_has_message: %s', async (kind, text) => {
    await failedJob(kind, 'no rule for Foo');
    expect(screen.getByRole('alert')).toHaveTextContent(text);
    expect(screen.getByRole('button', { name: 'Choose another file' })).toBeInTheDocument();
  });
});

describe('shell', () => {
  it('trademark_footer_present', async () => {
    await open();
    expect(
      screen.getByText(
        'Revit is a registered trademark of Autodesk, Inc. Backsave is not affiliated with or endorsed by Autodesk.',
      ),
    ).toBeInTheDocument();
  });

  it('links to the file handling page and the source code, and switches language', async () => {
    const { i18n } = await open();
    expect(screen.getByRole('link', { name: 'How your files are handled' })).toHaveAttribute('href', '/how-it-works');
    const source = screen.getByRole('link', { name: 'Source code' });
    expect(source).toHaveAttribute('href', 'https://github.com/PhanCongVuDuc/backsave-revit');
    expect(source).toHaveAttribute('rel', 'noopener');
    expect(screen.getByRole('button', { name: 'EN' })).toHaveAttribute('aria-pressed', 'true');
    await user().click(screen.getByRole('button', { name: 'VI' }));
    expect(i18n.global.locale.value).toBe('vi');
    expect(screen.getByRole('button', { name: viConvert.convert.add.button })).toBeInTheDocument();
  });

  it('says the tool is open source, with a link to the code', async () => {
    await open();
    expect(screen.getByText(/Backsave is open source/)).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'github.com/PhanCongVuDuc/backsave-revit' })).toHaveAttribute(
      'href',
      'https://github.com/PhanCongVuDuc/backsave-revit',
    );
  });

  it('about_page_says_how_to_have_a_report_deleted_and_who_made_it', async () => {
    await open('/how-it-works');
    expect(screen.getByRole('link', { name: 'phancongvuduc@gmail.com' })).toHaveAttribute(
      'href',
      'mailto:phancongvuduc@gmail.com',
    );
    expect(screen.getByRole('link', { name: 'GitHub issues' })).toHaveAttribute(
      'href',
      'https://github.com/PhanCongVuDuc/backsave-revit/issues',
    );
    const linkedin = screen.getByRole('link', { name: 'LinkedIn' });
    expect(linkedin).toHaveAttribute('href', 'https://www.linkedin.com/in/phancongvuduc/');
    expect(linkedin).toHaveAttribute('rel', 'noopener');
    expect(screen.getByText(/Made by Phan Cong Vu Duc/)).toBeInTheDocument();
  });

  it('about_page_lists_the_deletion_times', async () => {
    await open('/how-it-works');
    expect(screen.getByRole('heading', { level: 1 })).toHaveTextContent('How your files are handled');
    expect(screen.getAllByRole('row')).toHaveLength(7);
    expect(screen.getByRole('link', { name: /cleanup\.rs/ })).toHaveAttribute(
      'href',
      'https://github.com/PhanCongVuDuc/backsave-revit/blob/main/server/src/cleanup.rs',
    );
    expect(screen.getByRole('link', { name: /reports\.rs/ })).toHaveAttribute(
      'href',
      'https://github.com/PhanCongVuDuc/backsave-revit/blob/main/server/src/reports.rs',
    );
  });
});

describe('reopening', () => {
  it('asks for the same file to continue an unfinished upload', async () => {
    mocked.getJob.mockResolvedValueOnce(status({ state: 'waiting_upload', name: 'tower.rvt', size: 1 }));
    await open(`/job/${JOB}`);
    expect(
      screen.getByText('An upload of tower.rvt did not finish. Choose the same file to continue.'),
    ).toBeInTheDocument();
    await user().upload(screen.getByLabelText('Revit file'), revitFile(2026, 'other.rvt'));
    await settle();
    expect(screen.getByText('This is not the same file. Choose tower.rvt again, or start over.')).toBeInTheDocument();
    await user().click(screen.getByRole('button', { name: 'Start over' }));
    await settle();
    expect(mocked.deleteJob).toHaveBeenCalledWith(JOB);
    expect(screen.queryByText(/did not finish/)).not.toBeInTheDocument();
  });

  it('shows the versions of a reopened running job', async () => {
    mocked.getJob.mockResolvedValue(status({ state: 'running', elapsedS: 5, year: 2026, target: 2024 }));
    await open(`/job/${JOB}`);
    expect(screen.getByText(/^Converting 2026 → 2024/)).toBeInTheDocument();
  });

  it('explains a full disk after the upload broke', async () => {
    mocked.getJob.mockResolvedValue(
      status({ state: 'failed', error: { kind: 'disk_full', message: 'The server could not store the file.' } }),
    );
    await open(`/job/${JOB}`);
    expect(screen.getByRole('alert')).toHaveTextContent(en.convert.failure.disk_full);
  });
});

describe('keyboard and drag behaviour', () => {
  it('moves focus into the card when the step changes', async () => {
    await open();
    await user().upload(screen.getByLabelText('Revit file'), revitFile());
    await settle();
    expect(screen.getByRole('combobox', { name: 'Target Revit version' })).toHaveFocus();
  });

  it('stops the browser opening a file dropped outside the zone, only while the zone is shown', async () => {
    await open();
    const stray = new Event('drop', { bubbles: true, cancelable: true });
    document.body.dispatchEvent(stray);
    expect(stray.defaultPrevented).toBe(true);

    await user().upload(screen.getByLabelText('Revit file'), revitFile());
    await settle();
    const later = new Event('drop', { bubbles: true, cancelable: true });
    document.body.dispatchEvent(later);
    expect(later.defaultPrevented).toBe(false);
  });
});
