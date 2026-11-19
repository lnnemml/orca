/**
 * The job detail's notices after a submit (ADR-024 o26.6): the general `notice` (the `%pal`
 * alignment, a cancel's answer) and, beside it, the submit-time memory and disk notice — two
 * banners, so neither overwrites the other.
 */
export function SubmitNoticeBanners({
  notice,
  memoryNotice,
}: {
  notice: { text: string; tone: "ok" | "warn" } | null;
  memoryNotice: string | null;
}) {
  return (
    <>
      {notice ? (
        <div className={`banner ${notice.tone}`} style={{ marginBottom: 10 }} data-testid="submit-notice">
          {notice.text}
        </div>
      ) : null}
      {memoryNotice ? (
        <div className="banner warn" style={{ marginBottom: 10 }} data-testid="memory-notice">
          {memoryNotice}
        </div>
      ) : null}
    </>
  );
}
