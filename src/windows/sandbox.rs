//! Implements Sandbox on the top of `constrain` and `isolate`
use crate::windows::{constrain::Job, isolate::Profile, Error};
use tracing::instrument;
#[derive(Debug)]
pub struct WindowsSandbox {
    pub(crate) job: Job,
    pub(crate) profile: Profile,
    pub(crate) id: String,
}

impl WindowsSandbox {
    #[instrument]
    pub(in crate::windows) fn create(options: crate::SandboxOptions) -> Result<Self, Error> {
        let id = crate::util::gen_jail_id();
        let mut job = Job::new(&id)?;
        let profile = Profile::new(&id)?;
        job.enable_resource_limits(&options)?;

        Ok(Self { job, id, profile })
    }
}

impl crate::Sandbox for WindowsSandbox {
    type Error = Error;

    fn id(&self) -> String {
        self.id.clone()
    }

    fn kill(&self) -> Result<(), Self::Error> {
        self.job.kill()
    }

    fn resource_usage(&self) -> Result<crate::ResourceUsageData, Self::Error> {
        self.job.resource_usage()
    }

    fn check_cpu_tle(&self) -> Result<bool, Self::Error> {
        self.job.check_cpu_tle()
    }

    fn check_real_tle(&self) -> Result<bool, Self::Error> {
        self.job.check_real_tle()
    }
}
