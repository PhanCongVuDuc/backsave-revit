import { render, screen } from '@testing-library/vue';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { createI18n } from 'vue-i18n';

import en from '../locales/en.json';
import viMessages from '../locales/vi.json';

import UnopenablePanel from './UnopenablePanel.vue';

interface Props {
  reportCode: string | null;
  canReuseFile: boolean;
  sending: boolean;
  progress: number;
  error: string | null;
}

const IDLE: Props = { reportCode: null, canReuseFile: true, sending: false, progress: 0, error: null };

function show(props: Partial<Props> = {}, locale: 'en' | 'vi' = 'en') {
  const i18n = createI18n({ legacy: false, locale, fallbackLocale: 'en', messages: { en, vi: viMessages } });
  return render(UnopenablePanel, { props: { ...IDLE, ...props }, global: { plugins: [i18n] } });
}

const text = en.convert.unopenable;

describe('UnopenablePanel', () => {
  it('keeps Send locked until the user consents, then sends the note and no file', async () => {
    const user = userEvent.setup();
    const view = show();
    const send = screen.getByRole('button', { name: text.send });
    const consent = screen.getByRole('checkbox', { name: text.consent });
    expect(consent).not.toBeChecked();
    expect(send).toBeDisabled();
    await user.click(consent);
    expect(send).toBeEnabled();
    await user.type(screen.getByRole('textbox', { name: text.note }), 'Revit says corrupt');
    await user.click(send);
    expect(view.emitted('send')).toEqual([['Revit says corrupt', undefined]]);
  });

  it('limits the note to 500 characters', () => {
    show();
    expect(screen.getByRole('textbox', { name: text.note })).toHaveAttribute('maxlength', '500');
  });

  it('hides the file picker while the tab still holds the file', () => {
    show({ canReuseFile: true });
    expect(screen.queryByLabelText(text.pick)).toBeNull();
  });

  it('asks for the original file again and sends the one picked', async () => {
    const user = userEvent.setup();
    const view = show({ canReuseFile: false });
    const picker = screen.getByLabelText(text.pick);
    await user.click(screen.getByRole('checkbox', { name: text.consent }));
    const send = screen.getByRole('button', { name: text.send });
    expect(send).toBeDisabled();
    const file = new File(['abcd'], 'tower.rvt');
    await user.upload(picker, file);
    expect(send).toBeEnabled();
    await user.click(send);
    expect(view.emitted('send')).toEqual([['', file]]);
  });

  it('shows the progress and locks Send while sending', () => {
    show({ sending: true, progress: 0.42 });
    expect(screen.getByRole('button', { name: 'Sending… 42%' })).toBeDisabled();
  });

  it('shows only the report code once sent', () => {
    show({ reportCode: 'R-7K2M9Q' });
    expect(screen.getByText(text.sent.replace('{code}', 'R-7K2M9Q'))).toBeInTheDocument();
    expect(screen.queryByRole('button')).toBeNull();
    expect(screen.queryByRole('checkbox')).toBeNull();
  });

  it('says when the file picked is not the original', () => {
    show({ canReuseFile: false, error: 'size_mismatch' });
    expect(screen.getByRole('alert')).toHaveTextContent(text.failed_size_mismatch);
    expect(screen.getByLabelText(text.pick)).toBeInTheDocument();
  });

  it('says it can no longer be reported once the job is gone, and points to the footer', () => {
    show({ error: 'expired' });
    expect(screen.getByRole('alert')).toHaveTextContent(text.failed_expired);
    expect(screen.queryByRole('button')).toBeNull();
  });

  it('shows an unknown refusal with its code', () => {
    show({ error: 'teapot' });
    expect(screen.getByRole('alert')).toHaveTextContent(text.failed.replace('{code}', 'teapot'));
  });

  it('speaks Vietnamese', () => {
    show({}, 'vi');
    expect(screen.getByRole('checkbox', { name: viMessages.convert.unopenable.consent })).toBeInTheDocument();
  });
});
