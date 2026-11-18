export const CREDENTIAL_ORIGIN_FLOOR = [
  'https://ims-na1.adobelogin.com',
  'https://ims-na1-stg1.adobelogin.com',
];

export const CATALOG_CREDENTIAL_ORIGINS = {
  origins: ['https://ims-na1.adobelogin.com', 'https://ims-na1-stg1.adobelogin.com'],
  refusal: 'slicc: refused: this origin holds sign-in credentials',
};

export const CREDENTIAL_ORIGINS = {
  origins: [...new Set([...CREDENTIAL_ORIGIN_FLOOR, ...CATALOG_CREDENTIAL_ORIGINS.origins])],
  refusal: CATALOG_CREDENTIAL_ORIGINS.refusal,
};
