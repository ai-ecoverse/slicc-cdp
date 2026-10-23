const sliccPickers = new Set([
  'color',
  'date',
  'datetime-local',
  'file',
  'month',
  'range',
  'time',
  'week',
]);

function sliccNormalize(text) {
  return String(text ?? '')
    .replace(/[\u200b\u00ad]/g, '')
    .trim()
    .replace(/\s+/g, ' ');
}

function sliccLower(text) {
  return sliccNormalize(text).toLowerCase();
}

function sliccDeep(event) {
  const path = typeof event.composedPath === 'function' ? event.composedPath() : [];
  return path.find((node) => node?.nodeType === 1) ?? event.target;
}

function sliccModifiers(event) {
  return (
    (event.altKey ? 1 : 0) |
    (event.ctrlKey ? 2 : 0) |
    (event.metaKey ? 4 : 0) |
    (event.shiftKey ? 8 : 0)
  );
}

function sliccType(node) {
  return String(node?.type ?? '').toLowerCase();
}

function sliccCheckbox(node) {
  return node?.nodeName === 'INPUT' && ['checkbox', 'radio'].includes(sliccType(node))
    ? node
    : null;
}

function sliccRange(node) {
  return node?.nodeName === 'INPUT' && sliccType(node) === 'range';
}

function sliccIgnoredMouse(node) {
  if (node.nodeName === 'SELECT' || node.nodeName === 'OPTION') return true;
  return node.nodeName === 'INPUT' && sliccPickers.has(sliccType(node));
}

function sliccEditable(node) {
  return node.nodeName === 'TEXTAREA' || node.nodeName === 'INPUT' || node.isContentEditable;
}

function sliccCount(list, test) {
  let found = 0;
  for (const item of list) {
    if (test(item)) found += 1;
    if (found > 1) return found;
  }
  return found;
}

function sliccSafe(read, fallback) {
  try {
    return read();
  } catch {
    return fallback;
  }
}

function sliccLabelText(element) {
  const labels = element.labels ? Array.from(element.labels) : [];
  return labels.length ? sliccNormalize(labels[0].textContent) : '';
}

function sliccOwnText(element, needle) {
  if (!sliccLower(element.textContent).includes(needle)) return false;
  return !Array.from(element.children).some((child) =>
    sliccLower(child.textContent).includes(needle)
  );
}

function sliccStableId(id) {
  return /^[A-Za-z][\w-]*$/.test(id) && !/\d{4,}/.test(id);
}

function sliccTestId(context) {
  const value = context.element.getAttribute('data-testid');
  if (!value) return null;
  const unique =
    sliccCount(context.all, (item) => item.getAttribute('data-testid') === value) === 1;
  return unique ? { kind: 'testid', value } : null;
}

function sliccRoleName(context) {
  const { own, label } = context;
  if (!own || !label || label.length > 80) return null;
  const needle = label.toLowerCase();
  const same = context.all.filter((item) => context.role(item) === own && context.visible(item));
  if (sliccCount(same, (item) => context.name(item).toLowerCase().includes(needle)) === 1)
    return { kind: 'role', role: own, name: label };
  if (sliccCount(same, (item) => context.name(item) === label) === 1)
    return { kind: 'role', role: own, name: label, exact: true };
  return null;
}

function sliccPlaceholder(context) {
  const value = sliccNormalize(context.element.getAttribute('placeholder'));
  if (!value) return null;
  const needle = value.toLowerCase();
  const matches = (item) => sliccLower(item.getAttribute('placeholder')).includes(needle);
  return sliccCount(context.all, matches) === 1 ? { kind: 'placeholder', value } : null;
}

function sliccLabel(context) {
  const value = sliccLabelText(context.element);
  if (!value) return null;
  const needle = value.toLowerCase();
  const matches = (item) => sliccLower(sliccLabelText(item)).includes(needle);
  return sliccCount(context.all, matches) === 1 ? { kind: 'label', value } : null;
}

function sliccText(context) {
  const value = sliccNormalize(context.element.textContent);
  if (sliccEditable(context.element) || !value || value.length > 80) return null;
  const needle = value.toLowerCase();
  return sliccCount(context.all, (item) => sliccOwnText(item, needle)) === 1
    ? { kind: 'text', value }
    : null;
}

function sliccBareRole(context) {
  const { own } = context;
  if (!own) return null;
  const matches = (item) => context.role(item) === own && context.visible(item);
  return sliccCount(context.all, matches) === 1 ? { kind: 'role', role: own } : null;
}

function sliccId(context) {
  const id = context.element.getAttribute('id');
  if (!id || !sliccStableId(id)) return null;
  const unique = sliccCount(context.all, (item) => item.getAttribute('id') === id) === 1;
  return unique ? { kind: 'css', value: `#${id}` } : null;
}

function sliccNth(context) {
  const { own, label } = context;
  if (!own || !label || label.length > 80) return null;
  const needle = label.toLowerCase();
  const same = context.all.filter(
    (item) =>
      context.role(item) === own &&
      context.visible(item) &&
      context.name(item).toLowerCase().includes(needle)
  );
  const index = same.indexOf(context.element);
  return index >= 0 ? { kind: 'role', role: own, name: label, nth: index } : null;
}

const sliccStrategies = [
  sliccTestId,
  sliccRoleName,
  sliccPlaceholder,
  sliccLabel,
  sliccText,
  sliccBareRole,
  sliccId,
  sliccNth,
];

function sliccLocate(api, element) {
  if (element?.nodeType !== 1) return { kind: 'css', value: 'body' };
  if (element === document.body || element === document.documentElement)
    return { kind: 'css', value: element.nodeName.toLowerCase() };
  const role = (item) => sliccSafe(() => api.getAriaRole(item) ?? '', '');
  const visible = (item) => sliccSafe(() => !api.isElementHiddenForAria(item), false);
  const name = (item) =>
    sliccSafe(() => {
      const found = api.getElementAccessibleName(item, false);
      return sliccNormalize(typeof found === 'string' ? found : found?.text);
    }, '');
  const own = role(element);
  const context = {
    element,
    all: Array.from(document.querySelectorAll('*')),
    role,
    visible,
    name,
    own,
    label: own ? name(element) : '',
  };
  for (const strategy of sliccStrategies) {
    const found = strategy(context);
    if (found) return found;
  }
  return { kind: 'css', value: api.cssPath(element) || element.nodeName.toLowerCase() };
}

function sliccBlockedKey(event, target) {
  if (event.key === 'Enter' && (target.nodeName === 'TEXTAREA' || target.isContentEditable))
    return true;
  if (
    ['Backspace', 'Delete', 'AltGraph', 'Shift', 'Control', 'Meta', 'Alt', 'Process'].includes(
      event.key
    )
  )
    return true;
  if (event.key === '@' && event.code === 'KeyL') return true;
  if (navigator.platform.includes('Mac')) return event.key === 'v' && event.metaKey;
  return (event.key === 'v' && event.ctrlKey) || (event.key === 'Insert' && event.shiftKey);
}

function sliccPressed(event, target) {
  if (typeof event.key !== 'string' || sliccBlockedKey(event, target)) return false;
  const modified = event.ctrlKey || event.altKey || event.metaKey;
  if (event.key.length === 1 && !modified) return !sliccEditable(target);
  return true;
}

function sliccButton(event) {
  if (event.button === 1) return 'middle';
  return event.button === 2 ? 'right' : 'left';
}

function sliccClickAction(event, target, forced) {
  const action = {
    name: 'click',
    button: forced ?? sliccButton(event),
    modifiers: sliccModifiers(event),
    clickCount: forced ? 1 : event.detail,
  };
  if (target.nodeName === 'CANVAS') action.position = { x: event.offsetX, y: event.offsetY };
  return action;
}

function sliccInputAction(target) {
  if (target.nodeName === 'INPUT' && sliccType(target) === 'file')
    return {
      name: 'setInputFiles',
      files: Array.from(target.files ?? []).map((file) => file.name),
    };
  if (sliccRange(target)) return { name: 'fill', text: String(target.value) };
  if (['INPUT', 'TEXTAREA'].includes(target.nodeName) || target.isContentEditable) {
    if (sliccCheckbox(target)) return null;
    return {
      name: 'fill',
      text: String(target.isContentEditable ? target.innerText : target.value),
    };
  }
  if (target.nodeName === 'SELECT')
    return {
      name: 'select',
      options: Array.from(target.selectedOptions).map((option) => option.value),
    };
  return null;
}

globalThis.sliccRecorder = (api) => {
  const queue = [];
  const waiters = [];
  const listeners = [];
  let polled = Date.now();
  const record = (action, element) => {
    if (Date.now() - polled > 5000) {
      queue.length = 0;
      return;
    }
    if (queue.length >= 200) return;
    try {
      api.beginAriaCaches();
      action.locator = sliccLocate(api, element);
    } catch {
      action.locator = { kind: 'css', value: 'body' };
    } finally {
      api.endAriaCaches();
    }
    queue.push(action);
    for (const wake of waiters.splice(0)) wake();
  };
  const click = (event, forced) => {
    const target = sliccDeep(event);
    if (!event.isTrusted || !target || sliccRange(target) || sliccIgnoredMouse(target)) return;
    if (!forced && event.detail === 0) return;
    const box = sliccCheckbox(target);
    if (box && event.detail === 1 && !forced) {
      record({ name: box.checked ? 'check' : 'uncheck' }, box);
      return;
    }
    const canvas = target.nodeName === 'CANVAS';
    const owner = canvas
      ? target
      : (target.closest('button,[role=button],a,[role=link]') ?? target);
    record(sliccClickAction(event, target, forced), owner);
  };
  const input = (event) => {
    const target = sliccDeep(event);
    if (!event.isTrusted || !target) return;
    const action = sliccInputAction(target);
    if (action) record(action, target);
  };
  const keydown = (event) => {
    const target = sliccDeep(event);
    if (!event.isTrusted || !target || !sliccPressed(event, target)) return;
    record({ name: 'press', key: event.key, modifiers: sliccModifiers(event) }, target);
  };
  const listen = (type, listener) => {
    document.addEventListener(type, listener, true);
    listeners.push([type, listener]);
  };
  listen('click', (event) => click(event));
  listen('auxclick', (event) => {
    if (event.button === 1) click(event, 'middle');
  });
  listen('contextmenu', (event) => click(event, 'right'));
  listen('input', input);
  listen('keydown', keydown);
  const dispose = () => {
    for (const [type, listener] of listeners.splice(0))
      document.removeEventListener(type, listener, true);
    queue.length = 0;
    for (const wake of waiters.splice(0)) wake();
  };
  const next = (wait) =>
    new Promise((resolve) => {
      polled = Date.now();
      if (queue.length) {
        resolve(queue.splice(0));
        return;
      }
      const wake = () => {
        clearTimeout(timer);
        resolve(queue.splice(0));
      };
      const timer = setTimeout(() => {
        const index = waiters.indexOf(wake);
        if (index >= 0) waiters.splice(index, 1);
        resolve(queue.splice(0));
      }, wait);
      waiters.push(wake);
    });
  return { href: () => location.href, next, dispose };
};
