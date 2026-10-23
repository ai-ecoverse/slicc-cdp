(function (mode, args) {
  const key = Symbol.for('slicc-landing');
  const up = (node) => (node instanceof ShadowRoot ? node.host : node.parentNode);
  const focused = () => {
    let node = document.activeElement;
    while (node?.shadowRoot?.activeElement) node = node.shadowRoot.activeElement;
    return node;
  };
  const owners = [HTMLIFrameElement, HTMLFrameElement, HTMLObjectElement, HTMLEmbedElement];
  const owner = (node) =>
    owners.some((kind) => node instanceof kind) || node?.localName === 'fencedframe';
  const fixed = /^(checkbox|radio|button|submit|reset|file|image|color|range|hidden)$/;
  const field = (node) =>
    node instanceof HTMLTextAreaElement ||
    (node instanceof HTMLInputElement && !fixed.test(node.type));
  const editable = (node) =>
    !!node && (node.isContentEditable || (field(node) && !node.readOnly && !node.disabled));
  const content = (node) => (node.isContentEditable ? node.textContent : node.value);
  const edit = (node, command, text) => {
    const before = content(node);
    document.execCommand(command, false, text);
    return content(node) !== before;
  };
  const held = (mask) => ({
    altKey: !!(mask & 1),
    ctrlKey: !!(mask & 2),
    metaKey: !!(mask & 4),
    shiftKey: !!(mask & 8),
  });
  const arm = () => {
    globalThis[key]?.stop();
    const seen = [];
    const record = (event) => {
      if (event.isTrusted) seen.push(event.type);
    };
    for (const type of args.types) addEventListener(type, record, true);
    globalThis[key] = {
      seen,
      stop: () => {
        for (const type of args.types) removeEventListener(type, record, true);
      },
    };
    return 'armed';
  };
  const read = () => {
    const probe = globalThis[key];
    delete globalThis[key];
    probe?.stop();
    return probe ? probe.seen : null;
  };
  const activatable = 'a[href],area[href],button,input,select,summary,[role=button]';
  const erasers = { Backspace: 'delete', Delete: 'forwardDelete' };
  const fallback = (target, typed) => {
    if (typed) return edit(target, 'insertText', args.text);
    if (target.form) return target.form.requestSubmit();
    if (!(target instanceof HTMLInputElement))
      edit(target, target.isContentEditable ? 'insertParagraph' : 'insertLineBreak');
  };
  const press = (node) => {
    const enter = args.key === 'Enter';
    const erase = erasers[args.key];
    const typed = !!args.text && !enter && args.key !== 'Tab' && !(args.modifiers & 6);
    const target = node ?? document.body;
    const activates =
      (enter || args.key === ' ') && !editable(target) && !!target.closest(activatable);
    if (activates || !(typed || enter || erase || args.key === 'Escape')) return 'unsupported';
    const init = {
      key: args.key,
      code: args.code,
      bubbles: true,
      cancelable: true,
      composed: true,
      view: window,
      ...held(args.modifiers),
    };
    const go = target.dispatchEvent(new KeyboardEvent('keydown', init)) && editable(target);
    if (go && erase) edit(target, erase);
    else if (
      go &&
      args.key !== 'Escape' &&
      target.dispatchEvent(new KeyboardEvent('keypress', init))
    )
      fallback(target, typed);
    target.dispatchEvent(new KeyboardEvent('keyup', init));
    return 'pressed';
  };
  const deepest = (x, y) => {
    let hit = document.elementFromPoint(x, y);
    let inner = hit?.shadowRoot?.elementFromPoint(x, y);
    while (inner && inner !== hit) {
      hit = inner;
      inner = hit.shadowRoot?.elementFromPoint(x, y);
    }
    return hit;
  };
  const choose = () => {
    const anchor = this instanceof Element ? this : document.querySelector(args.selector);
    const hit = deepest(args.x, args.y);
    for (let at = anchor; at; at = up(at)) if (at === hit) return anchor;
    return hit ?? anchor;
  };
  const focusable =
    'a[href],area[href],button,input,select,textarea,iframe,summary,[tabindex],[contenteditable]';
  const gesture = (target) => {
    const base = {
      bubbles: true,
      cancelable: true,
      composed: true,
      view: window,
      clientX: args.x,
      clientY: args.y,
      button: args.button,
      ...held(args.modifiers),
    };
    const pointer = { pointerId: 1, pointerType: 'mouse', isPrimary: true };
    const fire = (type, detail, buttons, Kind = MouseEvent, extra = {}) =>
      target.dispatchEvent(new Kind(type, { ...base, detail, buttons, ...extra }));
    const move = () => {
      const chain = [];
      for (let at = target; at; at = up(at)) if (at instanceof Element) chain.unshift(at);
      const quiet = { ...base, bubbles: false, cancelable: false, button: 0, buttons: 0 };
      fire('pointerover', 0, 0, PointerEvent, { ...pointer, button: -1 });
      fire('mouseover', 0, 0, MouseEvent, { button: 0 });
      for (const at of chain) at.dispatchEvent(new MouseEvent('mouseenter', quiet));
      fire('pointermove', 0, 0, PointerEvent, { ...pointer, button: -1 });
      fire('mousemove', 0, 0, MouseEvent, { button: 0 });
    };
    const down = (detail) => {
      fire('pointerdown', 0, args.buttons, PointerEvent, pointer);
      if (!fire('mousedown', detail, args.buttons)) return;
      const next = target.closest(focusable);
      if (next && !next.disabled) next.focus({ preventScroll: true });
      else document.activeElement?.blur();
    };
    const release = (detail) => {
      fire('pointerup', 0, 0, PointerEvent, pointer);
      fire('mouseup', detail, 0);
    };
    const now = { mousemove: move, mousedown: down, mouseup: release };
    const later = [];
    for (const [type, detail] of args.steps) {
      if (now[type]) now[type](detail);
      else later.push([type, detail]);
    }
    const channel = new MessageChannel();
    channel.port1.onmessage = () => {
      channel.port1.close();
      for (const [type, detail] of later) fire(type, detail, 0);
    };
    channel.port2.postMessage(0);
    return 'done';
  };
  const point = () => {
    const target = choose();
    if (!target?.isConnected) return 'missing';
    return owner(target) ? 'frame' : gesture(target);
  };
  const near = (run) => () => {
    const node = focused();
    return owner(node) ? 'frame' : run(node);
  };
  const modes = {
    visible: () => document.visibilityState,
    arm,
    read,
    focus: near((node) => (editable(node) ? 'editable' : 'none')),
    insert: near((node) => (edit(node, 'insertText', args.text) ? 'inserted' : 'unchanged')),
    key: near(press),
    pointer: point,
  };
  return modes[mode]();
});
