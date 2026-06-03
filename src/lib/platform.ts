// Centralized platform detection. iOS WebViews report iPhone/iPad/iPod in the user
// agent (not the literal "ios"), so they must be matched explicitly.
const ua = typeof navigator !== 'undefined' ? navigator.userAgent : '';

export const isAndroid = /android/i.test(ua);
export const isIOS = /iphone|ipad|ipod/i.test(ua);
export const isMobile = isAndroid || isIOS;
