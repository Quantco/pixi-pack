// A window with a progress bar, shown by pixi-launcher on macOS while setting up an application.
// Usage: osascript -l JavaScript progress-window.js <title> <text> <status file>
// The status file holds the fraction done and a status line, it is read periodically.
ObjC.import('Cocoa');

function run(argv) {
  const [title, text, statusFile] = argv;
  const app = $.NSApplication.sharedApplication;
  app.setActivationPolicy($.NSApplicationActivationPolicyRegular);

  const window = $.NSWindow.alloc.initWithContentRectStyleMaskBackingDefer(
    $.NSMakeRect(0, 0, 420, 110), $.NSWindowStyleMaskTitled, $.NSBackingStoreBuffered, false);
  window.title = title;
  const content = window.contentView;

  const label = $.NSTextField.wrappingLabelWithString(text);
  label.frame = $.NSMakeRect(20, 62, 380, 34);
  content.addSubview(label);

  const bar = $.NSProgressIndicator.alloc.initWithFrame($.NSMakeRect(20, 36, 380, 20));
  bar.indeterminate = true;
  bar.minValue = 0;
  bar.maxValue = 1;
  bar.startAnimation(null);
  content.addSubview(bar);

  const status = $.NSTextField.labelWithString('');
  status.frame = $.NSMakeRect(20, 12, 380, 18);
  status.font = $.NSFont.systemFontOfSize($.NSFont.smallSystemFontSize);
  status.textColor = $.NSColor.secondaryLabelColor;
  content.addSubview(status);

  window.center;
  window.makeKeyAndOrderFront(null);
  app.activateIgnoringOtherApps(true);

  $.NSTimer.scheduledTimerWithTimeIntervalRepeatsBlock(0.2, true, () => {
    const contents = $.NSString.stringWithContentsOfFileEncodingError(
      statusFile, $.NSUTF8StringEncoding, null);
    if (contents.isNil()) return;
    const [fraction, line] = contents.js.split('\n');
    bar.indeterminate = false;
    bar.doubleValue = Number(fraction);
    status.stringValue = line || '';
  });
  app.run;
}
